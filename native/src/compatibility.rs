use crate::{
    Launcher,
    library::{self, CompatibilityBadge, CompatibilityStatus, Game},
};
use gpui::Context;
use raesir_backend::{
    model::{LibraryGame, Os},
    protondb::{self, SteamEdition, Tier},
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Ratings {
    by_app: HashMap<u32, Option<Tier>>,
    cached_by_app: HashMap<u32, Tier>,
    pending: HashSet<u32>,
    xbox_editions: HashMap<String, Option<SteamEdition>>,
    cached_xbox_editions: HashMap<String, SteamEdition>,
    pending_xbox: HashSet<String>,
    pub scan_offset: usize,
}

pub(crate) fn app_id(game: &LibraryGame) -> Option<u32> {
    (game.store == "steam")
        .then(|| game.id.parse().ok())
        .flatten()
        .filter(|id| *id > 0)
}

impl Ratings {
    pub fn card_badge(&self, game: &Game, host: Os) -> Option<(CompatibilityBadge, &'static str)> {
        let status = self.status(&game.source, &game.title, host)?;
        Some((status.badge?, status.label))
    }

    pub fn status(&self, game: &LibraryGame, title: &str, host: Os) -> Option<CompatibilityStatus> {
        use CompatibilityBadge::*;
        if host != Os::Linux {
            return None;
        }
        if game.store == "xbox" {
            return self.xbox_status(game, title, host);
        }
        let id = app_id(game)?;
        let tier = self
            .by_app
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.cached_by_app.get(&id).copied())?;
        let (badge, label) = match tier {
            Tier::Native => (Tick, "ProtonDB Native"),
            Tier::Platinum => (Tick, "ProtonDB Platinum"),
            Tier::Gold => (Happy, "ProtonDB Gold"),
            Tier::Silver => (Sick, "ProtonDB Silver"),
            Tier::Bronze => (Sick, "ProtonDB Bronze"),
            Tier::Borked => (Sad, "ProtonDB Borked"),
        };
        Some(CompatibilityStatus {
            badge: Some(badge),
            label,
        })
    }

    pub fn matches_badge(&self, game: &Game, host: Os, selected: CompatibilityBadge) -> bool {
        self.card_badge(game, host)
            .is_some_and(|(badge, _)| badge == selected)
    }

    pub fn checking(&self) -> bool {
        !self.pending.is_empty() || !self.pending_xbox.is_empty()
    }

    fn xbox_status(
        &self,
        game: &LibraryGame,
        title: &str,
        host: Os,
    ) -> Option<CompatibilityStatus> {
        let edition = self
            .xbox_editions
            .get(title)
            .copied()
            .unwrap_or_else(|| self.cached_xbox_editions.get(title).copied());
        library::xbox_compatibility(game, host, edition)
    }

    fn can_request(&self) -> bool {
        self.pending.len() + self.pending_xbox.len() < 4
    }

    fn needs_xbox_edition(&self, game: &LibraryGame, title: &str) -> bool {
        library::xbox_needs_steam_match(game)
            && title != game.id
            && !title.trim().is_empty()
            && !self.xbox_editions.contains_key(title)
            && !self.pending_xbox.contains(title)
    }
}

pub(crate) fn should_resort_visible(
    active_verification: bool,
    scrolled: bool,
    initial_cache: bool,
) -> bool {
    active_verification || (initial_cache && !scrolled)
}

pub fn load_cached(this: &Launcher, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux {
        return;
    }
    let backend = this.backend.clone();
    let task = cx.background_executor().spawn(async move {
        let ratings = protondb::cached_ratings(&backend.cache);
        let xbox = protondb::cached_xbox_editions(&backend.cache, &ratings);
        (ratings, xbox)
    });
    cx.spawn(async move |this, cx| {
        let (ratings, xbox) = task.await;
        this.update(cx, |this, cx| {
            if !this.closing && (!ratings.is_empty() || !xbox.is_empty()) {
                this.compatibility.cached_by_app = ratings;
                this.compatibility.cached_xbox_editions = xbox;
                if should_resort_visible(
                    this.active_verification().is_some(),
                    this.rows.logical_scroll_top().item_ix > 0,
                    true,
                ) {
                    this.update_visible_matches();
                }
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

pub fn request_filtered(this: &mut Launcher, cx: &mut Context<Launcher>) {
    let Some(selected) = this.active_verification() else {
        return;
    };
    while !this.closing && this.compatibility.can_request() {
        let Some(&index) = this.matching_games.get(this.compatibility.scan_offset) else {
            break;
        };
        this.compatibility.scan_offset += 1;
        // A Steam match can provide an Xbox compatibility hint, but never verifies the Xbox edition.
        if selected != CompatibilityBadge::Tick || this.games[index].source.store != "xbox" {
            request(this, index, cx);
        }
    }
}

pub fn request(this: &mut Launcher, index: usize, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux || this.closing || !this.compatibility.can_request() {
        return;
    }
    let game = &this.games[index];
    if this
        .compatibility
        .needs_xbox_edition(&game.source, &game.title)
    {
        request_xbox(this, game.source.id.clone(), game.title.clone(), cx);
        return;
    }
    if let Some(id) = app_id(&game.source) {
        request_steam(this, id, cx);
    }
}

fn request_steam(this: &mut Launcher, id: u32, cx: &mut Context<Launcher>) {
    if this.compatibility.by_app.contains_key(&id) || !this.compatibility.pending.insert(id) {
        return;
    }
    let backend = this.backend.clone();
    let task = cx
        .background_executor()
        .spawn(async move { protondb::rating(&backend.cache, id) });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        this.update(cx, |this, cx| {
            this.compatibility.pending.remove(&id);
            this.compatibility.by_app.insert(id, result);
            if should_resort_visible(
                this.active_verification().is_some(),
                this.rows.logical_scroll_top().item_ix > 0,
                false,
            ) {
                this.update_visible_matches();
            }
            if let Some(page) = &this.details
                && app_id(page.read(cx).game()) == Some(id)
            {
                page.update(cx, |page, cx| page.update_compatibility(this, cx));
            }
            request_filtered(this, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

pub fn request_details(this: &mut Launcher, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux || this.closing || !this.compatibility.can_request() {
        return;
    }
    let Some(game) = this.details.as_ref().map(|page| page.read(cx).game()) else {
        return;
    };
    if let Some(id) = app_id(game) {
        request_steam(this, id, cx);
    } else if this.compatibility.needs_xbox_edition(game, &game.title) {
        request_xbox(this, game.id.clone(), game.title.clone(), cx);
    }
}

fn request_xbox(
    this: &mut Launcher,
    product_id: String,
    title: String,
    cx: &mut Context<Launcher>,
) {
    this.compatibility.pending_xbox.insert(title.clone());
    let backend = this.backend.clone();
    let query = title.clone();
    let task = cx
        .background_executor()
        .spawn(async move { protondb::xbox_edition(&backend.cache, &product_id, &query) });
    cx.spawn(async move |this, cx| {
        let edition = task.await;
        this.update(cx, |this, cx| {
            this.compatibility.pending_xbox.remove(&title);
            this.compatibility.xbox_editions.insert(title, edition);
            if let Some(page) = &this.details {
                page.update(cx, |page, cx| page.update_compatibility(this, cx));
            }
            if edition.is_some()
                && should_resort_visible(
                    this.active_verification().is_some(),
                    this.rows.logical_scroll_top().item_ix > 0,
                    false,
                )
            {
                this.update_visible_matches();
            }
            request_filtered(this, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_compatibility_loads_never_reshuffle_unfiltered_grid() {
        assert!(!should_resort_visible(false, false, false));
        assert!(!should_resort_visible(false, true, false));
        assert!(should_resort_visible(false, false, true));
        assert!(!should_resort_visible(false, true, true));
        assert!(should_resort_visible(true, false, false));
        assert!(should_resort_visible(true, true, false));
    }
}

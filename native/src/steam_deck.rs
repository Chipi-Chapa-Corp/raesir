use crate::{
    Launcher,
    library::{self, CompatibilityBadge, Game, XboxCompatibility},
};
use raesir_backend::{
    model::{LibraryGame, Os},
    steam_deck::{self, Compatibility, SteamEdition},
};
use gpui::Context;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Ratings {
    by_app: HashMap<u32, Option<Compatibility>>,
    pending: HashSet<u32>,
    xbox_editions: HashMap<String, Option<SteamEdition>>,
    pending_xbox: HashSet<String>,
    pub scan_offset: usize,
}

fn app_id(game: &Game) -> Option<u32> {
    (game.source.store == "steam")
        .then(|| game.source.id.parse().ok())
        .flatten()
        .filter(|id| *id > 0)
}

impl Ratings {
    fn for_game(&self, game: &Game) -> Option<Compatibility> {
        self.by_app.get(&app_id(game)?).copied().flatten()
    }

    pub fn card_badge(&self, game: &Game, host: Os) -> Option<(CompatibilityBadge, &'static str)> {
        use CompatibilityBadge::*;
        if host != Os::Linux {
            return None;
        }
        if game.source.store == "xbox" {
            let status = self.xbox_status(&game.source, &game.title, host)?;
            return Some((status.badge?, status.label));
        }
        Some(match self.for_game(game)? {
            Compatibility::Verified => (Tick, "Steam Deck Verified"),
            Compatibility::Playable => (Happy, "Steam Deck Playable"),
            Compatibility::Unsupported => (Sad, "Steam Deck Unsupported"),
        })
    }

    pub fn matches_badge(&self, game: &Game, host: Os, selected: CompatibilityBadge) -> bool {
        self.card_badge(game, host)
            .is_some_and(|(badge, _)| badge == selected)
    }

    pub fn checking(&self) -> bool {
        !self.pending.is_empty() || !self.pending_xbox.is_empty()
    }

    pub fn xbox_status(
        &self,
        game: &LibraryGame,
        title: &str,
        host: Os,
    ) -> Option<XboxCompatibility> {
        let edition = self.xbox_editions.get(title).copied().flatten();
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

pub fn request_filtered(this: &mut Launcher, cx: &mut Context<Launcher>) {
    let Some(selected) = this.active_verification() else {
        return;
    };
    while !this.closing && this.steam_deck.can_request() {
        let Some(&index) = this.matching_games.get(this.steam_deck.scan_offset) else {
            break;
        };
        this.steam_deck.scan_offset += 1;
        // A Steam match can provide an Xbox compatibility hint, but never verifies the Xbox edition.
        let xbox = this.games[index].source.store == "xbox";
        if !matches!(
            (selected, xbox),
            (CompatibilityBadge::Tick, true) | (CompatibilityBadge::Sick, false)
        ) {
            request(this, index, cx);
        }
    }
}

pub fn request(this: &mut Launcher, index: usize, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux || this.closing || !this.steam_deck.can_request() {
        return;
    }
    let game = &this.games[index];
    if this
        .steam_deck
        .needs_xbox_edition(&game.source, &game.title)
    {
        request_xbox(this, game.source.id.clone(), game.title.clone(), cx);
        return;
    }
    let Some(id) = app_id(game) else {
        return;
    };
    if this.steam_deck.by_app.contains_key(&id) || !this.steam_deck.pending.insert(id) {
        return;
    }
    let backend = this.backend.clone();
    let task = cx
        .background_executor()
        .spawn(async move { steam_deck::rating(&backend.cache, id) });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        this.update(cx, |this, cx| {
            this.steam_deck.pending.remove(&id);
            this.steam_deck.by_app.insert(id, result);
            if result.is_some() {
                this.update_verified_matches();
            }
            request_filtered(this, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

pub fn request_details(this: &mut Launcher, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux || this.closing || !this.steam_deck.can_request() {
        return;
    }
    let Some(game) = this.details.as_ref().map(|page| page.read(cx).game()) else {
        return;
    };
    if this.steam_deck.needs_xbox_edition(game, &game.title) {
        request_xbox(this, game.id.clone(), game.title.clone(), cx);
    }
}

fn request_xbox(
    this: &mut Launcher,
    product_id: String,
    title: String,
    cx: &mut Context<Launcher>,
) {
    this.steam_deck.pending_xbox.insert(title.clone());
    let backend = this.backend.clone();
    let query = title.clone();
    let task = cx
        .background_executor()
        .spawn(async move { steam_deck::xbox_edition(&backend.cache, &product_id, &query) });
    cx.spawn(async move |this, cx| {
        let edition = task.await;
        this.update(cx, |this, cx| {
            this.steam_deck.pending_xbox.remove(&title);
            this.steam_deck.xbox_editions.insert(title, edition);
            if let Some(page) = &this.details {
                page.update(cx, |page, cx| page.update_xbox_compatibility(this, cx));
            }
            if edition.is_some() {
                this.update_verified_matches();
            }
            request_filtered(this, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

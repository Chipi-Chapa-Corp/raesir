use crate::{Launcher, library::Game};
use exposed_games_backend::{
    model::Os,
    steam_deck::{self, Compatibility},
};
use gpui::Context;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Ratings {
    by_app: HashMap<u32, Option<Compatibility>>,
    pending: HashSet<u32>,
}

fn app_id(game: &Game) -> Option<u32> {
    (game.source.store == "steam")
        .then(|| game.source.id.parse().ok())
        .flatten()
        .filter(|id| *id > 0)
}

impl Ratings {
    pub fn for_game(&self, game: &Game) -> Option<Compatibility> {
        self.by_app.get(&app_id(game)?).copied().flatten()
    }
}

pub fn request(this: &mut Launcher, index: usize, cx: &mut Context<Launcher>) {
    if this.host != Os::Linux || this.closing || this.steam_deck.pending.len() >= 4 {
        return;
    }
    let Some(id) = app_id(&this.games[index]) else {
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
            cx.notify();
        })
        .ok();
    })
    .detach();
}

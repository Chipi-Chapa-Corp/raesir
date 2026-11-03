use gpui::RenderImage;
use raesir_backend::model::{LibraryGame, Os};
use std::sync::Arc;

pub struct ActionTooltip(pub String);

impl gpui::Render for ActionTooltip {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{prelude::*, *};
        crate::header::pill()
            .rounded(rems(0.8))
            .px(rems(0.85))
            .py(rems(0.55))
            .max_w(rems(26.))
            .whitespace_normal()
            .text_size(rems(0.95))
            .child(self.0.clone())
    }
}

pub fn xbox_runtime_notice(
    report: &raesir_backend::model::Report,
    scope: u8,
    store: Option<&str>,
) -> bool {
    if store != Some("xbox") {
        return false;
    }
    let linux_selected = match scope {
        1 => report.host_os == Os::Linux,
        2 => report.host_os == Os::Windows,
        _ => true,
    };
    linux_selected
        && report
            .linux_systems
            .iter()
            .any(|system| !system.xbox_runtime)
}

pub struct Game {
    pub key: Arc<str>,
    pub title: String,
    pub store: usize,
    pub source: LibraryGame,
    pub poster: Option<Arc<RenderImage>>,
    pub card_poster: Option<Arc<RenderImage>>,
    pub background: Option<Arc<RenderImage>>,
    pub background_checked: bool,
}

impl Game {
    pub fn new(source: LibraryGame) -> Option<Self> {
        let store = crate::STORES.iter().position(|s| *s == source.store)?;
        Some(Self {
            key: Arc::from(source.key.as_str()),
            title: source.title.clone(),
            store,
            source,
            poster: None,
            card_poster: None,
            background: None,
            background_checked: false,
        })
    }

    pub fn installed(&self, scope: u8, host: Os) -> u8 {
        if scope != 2
            && host != Os::Unknown
            && self
                .source
                .installations
                .iter()
                .any(|i| i.os == host && matches!(i.state.as_str(), "installed" | "external"))
        {
            return 1;
        }
        if scope != 1
            && self.source.installations.iter().any(|i| {
                i.os != host
                    && i.os != Os::Unknown
                    && matches!(i.state.as_str(), "installed" | "external")
            })
        {
            return 2;
        }
        0
    }

    pub fn other_os(&self, host: Os) -> Option<Os> {
        self.source
            .installations
            .iter()
            .find(|i| i.os != host && i.os != Os::Unknown && i.state != "files_missing")
            .map(|i| i.os)
    }

    pub fn duplicate_systems(&self, host: Os) -> Vec<Os> {
        if self.installed(1, host) != 1 {
            return Vec::new();
        }
        [Os::Linux, Os::Windows]
            .into_iter()
            .filter(|os| {
                *os != host
                    && self.source.installations.iter().any(|i| {
                        i.os == *os && matches!(i.state.as_str(), "installed" | "external")
                    })
            })
            .collect()
    }

    pub fn matches_access(&self, access: u8) -> bool {
        match access {
            0 => true,
            1 => {
                self.source.in_library
                    && !self.source.game_pass_only
                    && !self.source.is_demo
                    && !raesir_backend::catalog::demo_title(&self.title)
            }
            2 => self.source.game_pass,
            _ => false,
        }
    }

    pub fn matches(
        &self,
        query: &str,
        store: Option<usize>,
        status: u8,
        scope: u8,
        host: Os,
    ) -> bool {
        if store.is_some_and(|s| s != self.store) || !self.title.to_lowercase().contains(query) {
            return false;
        }
        if !query.is_empty() {
            return true;
        }
        let systems = &self.source.client_os;
        if scope == 1 && (host == Os::Unknown || !systems.contains(&host)) {
            return false;
        }
        if scope == 2 && !systems.iter().any(|os| *os != host && *os != Os::Unknown) {
            return false;
        }
        status == 0 || (status == 1) == (self.installed(scope, host) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raesir_backend::{catalog, model::Installation};

    #[test]
    fn xbox_notice_tracks_os_and_client_filters_and_each_linux_system() {
        let mut report: raesir_backend::model::Report = serde_json::from_value(serde_json::json!({
            "host_os":"linux","games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null,
            "linux_systems":[{"root":"/","xbox_runtime":true},{"root":"/mounted/linux","xbox_runtime":false}]
        })).unwrap();
        for (host, scope, shown) in [
            (Os::Linux, 0, true),
            (Os::Linux, 1, true),
            (Os::Linux, 2, false),
            (Os::Windows, 0, true),
            (Os::Windows, 1, false),
            (Os::Windows, 2, true),
        ] {
            report.host_os = host;
            assert!(!xbox_runtime_notice(&report, scope, None));
            assert_eq!(xbox_runtime_notice(&report, scope, Some("xbox")), shown);
            assert!(!xbox_runtime_notice(&report, scope, Some("steam")));
        }
        report.linux_systems[1].xbox_runtime = true;
        assert!(!xbox_runtime_notice(&report, 0, None));
        report.linux_systems.clear();
        assert!(!xbox_runtime_notice(&report, 0, None));
    }

    fn game(client: Vec<Os>, installed: Vec<(Os, &str)>) -> Game {
        let mut source = catalog::entry(
            "steam",
            "1",
            "Test game",
            installed
                .into_iter()
                .map(|(os, state)| Installation {
                    os,
                    state: state.into(),
                    library: "fixture".into(),
                    path: "fixture".into(),
                    last_seen: 0,
                })
                .collect(),
            "fixture",
        );
        source.client_os = client;
        source.in_library = true;
        Game::new(source).unwrap()
    }

    #[test]
    fn access_filter_separates_subscription_games_and_keeps_purchases_also_on_game_pass() {
        let mut game = game(vec![Os::Windows], vec![]);
        assert!(game.matches_access(0));
        assert!(game.matches_access(1));
        assert!(!game.matches_access(2));
        game.source.game_pass = true;
        game.source.game_pass_only = true;
        assert!(game.matches_access(0));
        assert!(!game.matches_access(1));
        assert!(game.matches_access(2));
        assert!(game.matches("test", None, 1, 1, Os::Linux));
        assert!(
            !game.matches_access(1),
            "search does not bypass the access filter"
        );
        game.source.game_pass_only = false;
        assert!(game.matches_access(1) && game.matches_access(2));
        game.source.is_demo = true;
        assert!(!game.matches_access(1));
        assert!(game.matches_access(0));
        game.source.is_demo = false;
        game.title = "Half-Life 2: Demo".into();
        assert!(
            !game.matches_access(1),
            "old inventories exclude demos before metadata refresh"
        );
        game.title = "Test game".into();
        game.source.in_library = false;
        assert!(
            !game.matches_access(1),
            "an installation does not prove ownership"
        );
    }

    #[test]
    fn os_filter_uses_clients_without_implying_installed() {
        let local = game(vec![Os::Linux], vec![]);
        assert!(local.matches("", None, 0, 1, Os::Linux));
        assert!(local.matches("", None, 2, 1, Os::Linux));
        assert!(!local.matches("", None, 1, 1, Os::Linux));
        let foreign = game(vec![Os::Windows], vec![]);
        assert!(!foreign.matches("", None, 0, 1, Os::Linux));
        assert!(foreign.matches("", None, 0, 2, Os::Linux));
    }

    #[test]
    fn search_ignores_status_and_os_but_keeps_provider_and_title() {
        for game in [
            game(vec![Os::Windows], vec![]),
            game(vec![Os::Linux], vec![(Os::Linux, "installed")]),
        ] {
            for status in 0..=2 {
                for scope in 0..=2 {
                    assert!(game.matches("test", None, status, scope, Os::Linux));
                    assert!(game.matches("test", Some(game.store), status, scope, Os::Linux));
                    assert!(!game.matches("test", Some(game.store + 1), status, scope, Os::Linux));
                    assert!(!game.matches("missing", None, status, scope, Os::Linux));
                }
            }
        }
    }

    #[test]
    fn foreign_install_is_gray_without_a_foreign_badge_in_this_os_filter() {
        let game = game(
            vec![Os::Linux, Os::Windows],
            vec![(Os::Windows, "installed")],
        );
        assert_eq!(game.installed(0, Os::Linux), 2);
        assert_eq!(game.installed(1, Os::Linux), 0);
        assert!(game.matches("", None, 0, 1, Os::Linux));
        assert!(!game.matches("", None, 1, 1, Os::Linux));
        assert!(game.matches("", None, 1, 2, Os::Linux));
    }

    #[test]
    fn dual_install_and_windows_host_have_correct_states() {
        let game = game(
            vec![Os::Linux, Os::Windows],
            vec![(Os::Linux, "external"), (Os::Windows, "installed")],
        );
        for host in [Os::Linux, Os::Windows] {
            assert_eq!(game.installed(0, host), 1);
            assert_eq!(game.installed(1, host), 1);
            assert_eq!(game.installed(2, host), 2);
            assert_ne!(game.other_os(host), Some(host));
            assert_eq!(
                game.duplicate_systems(host),
                vec![if host == Os::Linux {
                    Os::Windows
                } else {
                    Os::Linux
                }]
            );
        }
    }

    #[test]
    fn duplicate_badge_requires_installed_copies_on_both_known_systems() {
        for installations in [
            vec![(Os::Windows, "installed")],
            vec![(Os::Linux, "installed"), (Os::Windows, "incomplete")],
            vec![(Os::Linux, "installed"), (Os::Windows, "unavailable")],
            vec![(Os::Linux, "installed"), (Os::Unknown, "installed")],
            vec![(Os::Linux, "installed"), (Os::Linux, "installed")],
        ] {
            let game = game(vec![Os::Linux, Os::Windows], installations);
            assert!(game.duplicate_systems(Os::Linux).is_empty());
            assert!(game.duplicate_systems(Os::Unknown).is_empty());
        }
    }

    #[test]
    fn duplicate_systems_do_not_repeat_mount_aliases() {
        let game = game(
            vec![Os::Linux, Os::Windows],
            vec![
                (Os::Linux, "installed"),
                (Os::Windows, "installed"),
                (Os::Windows, "installed"),
            ],
        );
        assert_eq!(game.duplicate_systems(Os::Linux), vec![Os::Windows]);
    }

    #[test]
    fn unavailable_files_and_unknown_os_are_not_installed() {
        let game = game(
            vec![Os::Linux],
            vec![(Os::Linux, "unavailable"), (Os::Unknown, "installed")],
        );
        assert_eq!(game.installed(0, Os::Linux), 0);
        assert!(!game.matches("", None, 1, 0, Os::Linux));
    }

    #[test]
    fn remembered_foreign_install_keeps_its_os_badge_without_claiming_ready_files() {
        let game = game(
            vec![Os::Linux, Os::Windows],
            vec![(Os::Windows, "unavailable")],
        );
        assert_eq!(game.other_os(Os::Linux), Some(Os::Windows));
        assert_eq!(game.installed(0, Os::Linux), 0);
        assert!(!game.matches("", None, 1, 0, Os::Linux));
        assert!(game.matches("", None, 2, 2, Os::Linux));
    }
}

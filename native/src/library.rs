use gpui::RenderImage;
use raesir_lib::model::{LibraryGame, Os};
use std::sync::Arc;

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
    use raesir_lib::{catalog, model::Installation};

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
        Game::new(source).unwrap()
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

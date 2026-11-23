use raesir_backend::{
    model::{FerestreState, Installation, LibraryGame, Os},
    protondb::{SteamEdition, Tier},
};
use gpui::RenderImage;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityBadge {
    Tick,
    Happy,
    Sad,
    Sick,
}

impl CompatibilityBadge {
    pub const ALL: [Self; 4] = [Self::Tick, Self::Happy, Self::Sick, Self::Sad];

    pub fn key(self) -> &'static str {
        match self {
            Self::Tick => "verified",
            Self::Happy => "playable",
            Self::Sick => "no-steam-match",
            Self::Sad => "unsupported",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tick => "Verified, ProtonDB Native or Platinum",
            Self::Happy => "Playable or ProtonDB Gold",
            Self::Sick => "ProtonDB Silver or Bronze · Xbox without a Steam match",
            Self::Sad => "Unsupported or ProtonDB Borked",
        }
    }

    pub fn icon(self) -> gpui::Svg {
        use gpui::{Styled, rgb};
        let (glyph, color) = match self {
            Self::Tick => ("verified-symbolic", 0x9ff4bc),
            Self::Happy => ("face-smile-big-symbolic", 0xf6d48b),
            Self::Sad => ("face-sad-symbolic", 0xff9c9c),
            Self::Sick => ("emote-sick", 0xd8d8d8),
        };
        crate::icons::icon(glyph).text_color(rgb(color))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompatibilityStatus {
    pub badge: Option<CompatibilityBadge>,
    pub label: &'static str,
}

pub fn xbox_needs_steam_match(game: &LibraryGame) -> bool {
    game.store == "xbox"
        && !game.xbox_console_only
        && !game.ferestre_unsupported_package
        && game
            .ferestre_test
            .as_ref()
            .is_none_or(|test| test.state == FerestreState::Untested)
}

pub fn xbox_compatibility(
    game: &LibraryGame,
    host: Os,
    steam: Option<SteamEdition>,
) -> Option<CompatibilityStatus> {
    use CompatibilityBadge::*;
    if game.store != "xbox" || game.xbox_console_only || host != Os::Linux {
        return None;
    }
    let (badge, label) = if game.ferestre_unsupported_package {
        (Some(Sad), "Can't install on Linux")
    } else {
        match game.ferestre_test.as_ref().map(|test| test.state) {
            Some(FerestreState::Playable) => (Some(Tick), "Verified on Linux"),
            Some(FerestreState::PlayableWithIssues) => (Some(Happy), "Playable with issues"),
            Some(FerestreState::Menus) => (Some(Sad), "Menus only"),
            Some(FerestreState::Broken) => (Some(Sad), "Doesn't run on Linux"),
            None | Some(FerestreState::Untested) => match steam {
                Some(SteamEdition::Listed(Some(Tier::Borked))) => {
                    (Some(Sad), "Steam edition Borked on ProtonDB · Xbox untested")
                }
                Some(SteamEdition::Listed(Some(Tier::Silver))) => {
                    (Some(Sick), "Steam edition ProtonDB Silver · Xbox untested")
                }
                Some(SteamEdition::Listed(Some(Tier::Bronze))) => {
                    (Some(Sick), "Steam edition ProtonDB Bronze · Xbox untested")
                }
                Some(SteamEdition::Listed(Some(_))) => {
                    (Some(Happy), "Steam edition runs according to ProtonDB · Xbox untested")
                }
                Some(SteamEdition::Missing) => (Some(Sick), "Xbox untested · No Steam match"),
                Some(SteamEdition::Listed(None)) => (None, "Xbox untested · No ProtonDB rating"),
                None => (None, "Untested on Linux"),
            },
        }
    };
    Some(CompatibilityStatus { badge, label })
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FerestreSetup {
    Install,
    SignIn,
    Runtime,
    MaintainCli,
    MaintainRuntime,
}

pub fn ferestre_setup(
    report: &raesir_backend::model::Report,
    scope: u8,
    store: Option<&str>,
) -> Option<FerestreSetup> {
    if store != Some("xbox")
        || scope == 2
        || report.host_os != Os::Linux
        || !raesir_backend::ferestre::setup_supported()
    {
        return None;
    }
    let host = report
        .linux_systems
        .iter()
        .find(|system| system.root == "/")?;
    if host.managed_ferestre && !host.ferestre_runtime {
        Some(FerestreSetup::Runtime)
    } else if (host.managed_ferestre || host.ferestre_client) && !host.ferestre_library {
        Some(FerestreSetup::SignIn)
    } else if !host.xbox_runtime {
        Some(FerestreSetup::Install)
    } else {
        None
    }
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
        if !raesir_backend::catalog::visible(&source) {
            return None;
        }
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
        if scope != 1 && self.foreign_installations(host).next().is_some() {
            return 2;
        }
        0
    }

    fn foreign_installations(&self, host: Os) -> impl Iterator<Item = &Installation> {
        self.source.installations.iter().filter(move |i| {
            i.os != host
                && i.os != Os::Unknown
                && matches!(i.state.as_str(), "installed" | "external")
        })
    }

    pub fn foreign_install_badge(&self, host: Os) -> Option<Os> {
        self.foreign_installations(host)
            .find(|i| i.os != Os::Windows || (i.state == "installed" && !i.path.is_empty()))
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

    fn available_on_host(&self, host: Os) -> bool {
        host != Os::Unknown
            && self.source.client_os.contains(&host)
            && (host != Os::Linux
                || !self.source.ferestre_unsupported_package
                || self.installed(1, host) == 1)
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
        if scope == 1 && !self.available_on_host(host) {
            return false;
        }
        if scope == 2
            && !(host != Os::Windows && self.source.ferestre_unsupported_package)
            && !systems.iter().any(|os| *os != host && *os != Os::Unknown)
        {
            return false;
        }
        status == 0 || (status == 1) == (self.installed(scope, host) != 0)
    }
}

pub(crate) fn sort_matches(
    matches: &mut [usize],
    games: &[Game],
    host: Os,
    verified: impl Fn(&Game) -> bool,
) {
    matches.sort_by_cached_key(|&index| {
        let game = &games[index];
        (
            !game.matches_access(1),
            game.installed(0, host) == 0,
            !verified(game),
            !game.available_on_host(host),
        )
    });
}

pub fn matching_games(
    games: &[Game],
    query: &str,
    filters: crate::backend::LibraryFilters,
    host: Os,
) -> Vec<usize> {
    let mut matches: Vec<_> = games
        .iter()
        .enumerate()
        .filter(|(_, game)| {
            game.matches_access(filters.access)
                && game.matches(query, filters.store, filters.status, filters.os, host)
        })
        .map(|(index, _)| index)
        .collect();
    let scope = if query.is_empty() { filters.os } else { 0 };
    if filters.store.is_some() || scope == 2 {
        return matches;
    }

    let title_key = |game: &Game| {
        (
            raesir_backend::catalog::title_key(&game.title),
            game.source.is_demo,
        )
    };
    let installable = |game: &Game| raesir_backend::actions::can_install(&game.source);
    let mut available_stores = HashMap::new();
    for &index in &matches {
        let game = &games[index];
        if game.title != game.source.id && (game.installed(1, host) == 1 || installable(game)) {
            let key = title_key(game);
            if !key.0.is_empty() {
                *available_stores.entry(key).or_insert(0_usize) |= 1 << game.store;
            }
        }
    }
    if available_stores.is_empty() {
        return matches;
    }
    matches.retain(|&index| {
        let game = &games[index];
        game.title == game.source.id
            || game.installed(scope, host) != 0
            || installable(game)
            || available_stores
                .get(&title_key(game))
                .is_none_or(|stores| stores & !(1 << game.store) == 0)
    });
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use raesir_backend::{
        catalog,
        model::FerestreTest,
    };

    #[test]
    fn xbox_compatibility_uses_steam_only_for_untested_editions() {
        use CompatibilityBadge::*;
        let mut game = catalog::entry("xbox", "9MT5NJ5W7B8Z", "Fixture", vec![], "test");
        assert_eq!(
            xbox_compatibility(&game, Os::Linux, None),
            Some(CompatibilityStatus {
                badge: None,
                label: "Untested on Linux",
            })
        );
        for (edition, badge) in [
            (
                SteamEdition::Listed(Some(Tier::Gold)),
                Some(Happy),
            ),
            (
                SteamEdition::Listed(Some(Tier::Platinum)),
                Some(Happy),
            ),
            (SteamEdition::Listed(Some(Tier::Silver)), Some(Sick)),
            (SteamEdition::Listed(Some(Tier::Bronze)), Some(Sick)),
            (
                SteamEdition::Listed(Some(Tier::Borked)),
                Some(Sad),
            ),
            (SteamEdition::Listed(None), None),
            (SteamEdition::Missing, Some(Sick)),
        ] {
            assert_eq!(
                xbox_compatibility(&game, Os::Linux, Some(edition))
                    .unwrap()
                    .badge,
                badge,
            );
        }
        game.ferestre_pc_package = true;
        for (state, badge, label) in [
            (FerestreState::Playable, Some(Tick), "Verified on Linux"),
            (
                FerestreState::PlayableWithIssues,
                Some(Happy),
                "Playable with issues",
            ),
            (FerestreState::Menus, Some(Sad), "Menus only"),
            (FerestreState::Broken, Some(Sad), "Doesn't run on Linux"),
            (
                FerestreState::Untested,
                Some(Happy),
                "Steam edition runs according to ProtonDB · Xbox untested",
            ),
        ] {
            game.ferestre_test = Some(FerestreTest {
                state,
                last_verified: Some("2026-11-22".into()),
                summary: None,
            });
            assert_eq!(
                xbox_compatibility(
                    &game,
                    Os::Linux,
                    Some(SteamEdition::Listed(Some(Tier::Platinum)))
                ),
                Some(CompatibilityStatus { badge, label }),
            );
        }
        assert!(xbox_compatibility(&game, Os::Windows, None).is_none());
        game.ferestre_unsupported_package = true;
        assert_eq!(
            xbox_compatibility(&game, Os::Linux, None),
            Some(CompatibilityStatus {
                badge: Some(Sad),
                label: "Can't install on Linux",
            })
        );
        game.xbox_console_only = true;
        assert!(xbox_compatibility(&game, Os::Linux, None).is_none());
        assert!(Game::new(game).is_none());
    }

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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn ferestre_setup_targets_only_the_current_linux_installation() {
        let mut report: raesir_backend::model::Report =
            serde_json::from_value(serde_json::json!({
                "host_os":"linux","games":[],"libraries":[],"volumes":[],"warnings":[],
                "owned_account":null,
                "linux_systems":[{"root":"/","xbox_runtime":false},
                                 {"root":"/mounted/linux","xbox_runtime":true}]
            }))
            .unwrap();
        assert!(ferestre_setup(&report, 0, None).is_none());
        assert!(ferestre_setup(&report, 2, Some("xbox")).is_none());
        assert!(ferestre_setup(&report, 1, Some("xbox")) == Some(FerestreSetup::Install));
        report.linux_systems[0].xbox_runtime = true;
        assert!(ferestre_setup(&report, 0, Some("xbox")).is_none());
        report.linux_systems[0].managed_ferestre = true;
        assert!(ferestre_setup(&report, 0, Some("xbox")) == Some(FerestreSetup::Runtime));
        report.linux_systems[0].ferestre_runtime = true;
        assert!(ferestre_setup(&report, 0, Some("xbox")) == Some(FerestreSetup::SignIn));
        report.linux_systems[0].ferestre_runtime = false;
        report.linux_systems[0].ferestre_library = true;
        assert!(ferestre_setup(&report, 0, Some("xbox")) == Some(FerestreSetup::Runtime));
        report.linux_systems[0].ferestre_runtime = true;
        assert!(ferestre_setup(&report, 0, Some("xbox")).is_none());
        report.host_os = Os::Windows;
        assert!(ferestre_setup(&report, 0, Some("xbox")).is_none());
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
    fn library_sort_uses_each_priority_in_order_and_keeps_ties_stable() {
        let local = Os::Linux;
        let foreign = Os::Windows;
        let mut games = [
            game(vec![local], vec![(local, "installed")]),
            game(vec![local], vec![(local, "incomplete")]),
            game(vec![local], vec![(local, "installed")]),
            game(vec![foreign], vec![(foreign, "installed")]),
            game(vec![local], vec![(local, "external")]),
            game(vec![local], vec![(local, "installed")]),
        ];
        games[0].source.game_pass_only = true;
        for index in [0, 1, 3, 4, 5] {
            games[index].title = "Verified".into();
        }
        let mut indices = [0, 1, 2, 3, 4, 5];
        sort_matches(&mut indices, &games, local, |game| game.title == "Verified");
        assert_eq!(indices, [4, 5, 3, 2, 1, 0]);
    }

    #[test]
    fn combined_library_hides_unavailable_copies_only_with_an_eligible_cross_store_option() {
        use raesir_backend::{
            model::{ActionTarget, GameAction},
            steam_control::Operation,
        };

        let host = Os::host();
        let foreign = if host == Os::Windows {
            Os::Linux
        } else {
            Os::Windows
        };
        let mut steam = game(vec![host], vec![]);
        steam.title = "Five Nights at Freddy's".into();
        steam.source.install = Some(GameAction {
            client: "Steam".into(),
            note: None,
            target: ActionTarget::Steam {
                root: "fixture".into(),
                appid: 1,
                operation: Operation::Install,
            },
        });
        let mut xbox = catalog::entry(
            "xbox",
            "example",
            "Five Nights at Freddy’s®",
            vec![],
            "fixture",
        );
        xbox.in_library = true;
        xbox.game_pass = true;
        xbox.client_os = vec![foreign];
        let xbox = Game::new(xbox).unwrap();
        let mut games = [steam, xbox];
        let filters = crate::backend::LibraryFilters {
            status: 0,
            os: 0,
            store: None,
            access: 0,
            verification: None,
        };
        let matching = |games: &[Game], filters| matching_games(games, "", filters, host);

        assert_eq!(matching(&games, filters), [0]);
        assert_eq!(matching_games(&games, "five nights", filters, host), [0]);
        assert_eq!(
            matching(
                &games,
                crate::backend::LibraryFilters {
                    store: Some(games[1].store),
                    ..filters
                }
            ),
            [1]
        );
        assert_eq!(
            matching(
                &games,
                crate::backend::LibraryFilters {
                    access: 2,
                    ..filters
                }
            ),
            [1]
        );
        assert_eq!(
            matching(&games, crate::backend::LibraryFilters { os: 2, ..filters }),
            [1]
        );

        games[1].title.push_str(" 2");
        assert_eq!(matching(&games, filters), [0, 1]);
        games[1].title = games[0].title.clone();
        games[1].source.is_demo = true;
        assert_eq!(matching(&games, filters), [0, 1]);
        games[1].source.is_demo = false;

        games[1].source.installations = game(vec![foreign], vec![(foreign, "installed")])
            .source
            .installations;
        assert_eq!(matching(&games, filters), [0, 1]);
        games[1].source.installations.clear();
        games[0].source.installations = game(vec![host], vec![(host, "installed")])
            .source
            .installations;
        assert_eq!(matching(&games, filters), [0]);
        games[0].source.install = None;
        games[0].source.installations.clear();
        assert_eq!(matching(&games, filters), [0, 1]);
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
    fn unsupported_xbox_pc_packages_are_visible_under_other_os_on_linux() {
        let mut xbox = game(vec![Os::Linux], vec![]);
        xbox.source.ferestre_unsupported_package = true;
        assert!(xbox.matches("", None, 0, 0, Os::Linux));
        assert!(!xbox.matches("", None, 0, 1, Os::Linux));
        assert!(xbox.matches("", None, 0, 2, Os::Linux));
        assert!(!xbox.matches("", None, 1, 2, Os::Linux));

        let mut installed = game(vec![Os::Linux], vec![(Os::Linux, "installed")]);
        installed.source.ferestre_unsupported_package = true;
        assert!(installed.matches("", None, 1, 1, Os::Linux));
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
        assert_eq!(game.foreign_install_badge(Os::Linux), Some(Os::Windows));
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
            assert_ne!(game.foreign_install_badge(host), Some(host));
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
    fn unverified_foreign_install_does_not_show_an_installed_badge() {
        for state in ["unavailable", "files_missing", "incomplete"] {
            let game = game(vec![Os::Linux, Os::Windows], vec![(Os::Windows, state)]);
            assert_eq!(game.foreign_install_badge(Os::Linux), None);
            assert_eq!(game.installed(0, Os::Linux), 0);
            assert!(!game.matches("", None, 1, 0, Os::Linux));
            assert!(game.matches("", None, 2, 2, Os::Linux));
        }
    }

    #[test]
    fn windows_badge_requires_a_confirmed_installation_path() {
        let mut game = game(vec![Os::Windows], vec![(Os::Windows, "external")]);
        game.source.installations[0].path.clear();
        assert_eq!(game.foreign_install_badge(Os::Linux), None);
        let mut installed = game.source.installations[0].clone();
        installed.state = "installed".into();
        game.source.installations.push(installed);
        assert_eq!(game.foreign_install_badge(Os::Linux), None);
        game.source.installations[1].path = "/mnt/windows/Game".into();
        assert_eq!(game.foreign_install_badge(Os::Linux), Some(Os::Windows));
        assert_eq!(game.foreign_install_badge(Os::Windows), None);
    }
}

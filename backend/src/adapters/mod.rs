mod epic_beta;
mod third_party;
mod ubisoft;
mod wargaming;
mod xbox;
use crate::{
    catalog,
    discovery::{self, Location},
    model::*,
};
use std::{collections::BTreeSet, path::Path};
pub(crate) use ubisoft::cached_cover as ubisoft_cover;
pub(crate) use ubisoft::hydrate_game_pass;
pub(crate) use wargaming::steam_app as wargaming_steam_app;
pub(crate) use wargaming::store_url as wargaming_store_url;

pub struct Scan<'a> {
    pub report: &'a Report,
    pub homes: Vec<Location>,
    pub prefixes: Vec<Location>,
    pub roots: Vec<Location>,
    pub games: Vec<LibraryGame>,
    pub verified: BTreeSet<String>,
    pub warnings: Vec<String>,
}

pub fn scan(
    report: &Report,
    previous: Option<&Report>,
    prefixes: Vec<Location>,
) -> (Vec<LibraryGame>, Vec<String>) {
    let mut scan = Scan {
        report,
        homes: discovery::homes(report),
        prefixes,
        roots: discovery::roots(report),
        games: catalog::from_report(report),
        verified: BTreeSet::new(),
        warnings: vec![],
    };
    epic_beta::scan(&mut scan);
    ubisoft::scan(&mut scan);
    xbox::scan(&mut scan);
    wargaming::scan(&mut scan);
    third_party::scan(&mut scan);
    if let Some(previous) = previous {
        retain(&mut scan, previous);
    }
    scan.games.retain(catalog::visible);
    scan.games
        .sort_by_cached_key(|g| (g.title.to_lowercase(), g.key.clone()));
    (scan.games, scan.warnings)
}

pub fn file(
    scan: &mut Scan<'_>,
    path: &Path,
    read: impl FnOnce(&mut Scan<'_>) -> Result<(), String>,
) {
    if !path.exists() {
        return;
    }
    let before = scan.games.clone();
    match read(scan) {
        Ok(()) => {
            scan.verified.insert(crate::epic::key(path));
        }
        Err(e) => {
            scan.games = before;
            scan.warnings.push(format!("{}: {e}", path.display()));
        }
    }
}

pub fn text<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}

pub fn image(v: &serde_json::Value) -> Option<String> {
    for key in ["art_cover", "coverUrl", "boxArt", "imageUrl", "cover"] {
        if let Some(value) = text(v, key) {
            return Some(value.into());
        }
    }
    let meta = v.get("metadata").unwrap_or(v);
    let images = meta.get("keyImages")?.as_array()?;
    images
        .iter()
        .find(|v| text(v, "type") == Some("DieselGameBoxTall"))
        .or_else(|| images.first())
        .and_then(|v| text(v, "url"))
        .map(str::to_owned)
}

fn retain(scan: &mut Scan<'_>, previous: &Report) {
    let core: BTreeSet<_> = previous
        .games
        .iter()
        .flat_map(|g| &g.installations)
        .chain(previous.epic.games.iter().flat_map(|g| &g.installations))
        .chain(previous.ea.games.iter().flat_map(|g| &g.installations))
        .map(|i| (&i.library, &i.path))
        .collect();
    for old in &previous.catalog {
        let mut game = old.clone();
        game.installations.retain(|i| {
            !core.contains(&(&i.library, &i.path)) && !scan.verified.contains(&i.library)
        });
        let membership = old.in_library
            && !["steam", "epic", "ea"].contains(&old.source.as_str())
            && !scan.verified.contains(&old.source);
        if game.installations.is_empty() && !membership {
            continue;
        }
        if !membership {
            game.client_os.clear();
            for install in &game.installations {
                catalog::add_os(&mut game.client_os, install.os);
            }
        }
        for i in &mut game.installations {
            i.state = "unavailable".into();
        }
        game.in_library = membership;
        game.launch = None;
        game.library = None;
        game.install = None;
        game.uninstall = None;
        catalog::merge(&mut scan.games, game);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn report() -> Report {
        crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        )
    }
    fn fixture<'a>(report: &'a Report, path: &Path) -> Scan<'a> {
        Scan {
            report,
            homes: vec![Location {
                path: path.into(),
                os: Os::Linux,
                local: true,
                client: String::new(),
                drive_c: None,
            }],
            prefixes: vec![],
            roots: vec![],
            games: vec![],
            verified: BTreeSet::new(),
            warnings: vec![],
        }
    }
    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    #[test]
    fn preview_reads_native_journal_and_excludes_aggregated_stores() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join(".local/share/com.epicgames.eglv2");
        let payload = dir.path().join("payload");
        write(&payload.join(".egstore/fixture.manifest"), "fixture");
        let journal = format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"version":1,"entries":2}),
            serde_json::json!({"installationId":"fixture","artifactId":"epic-fixture","namespace":"namespace","itemId":"catalog-id","complete":true,"dir":payload}),
            serde_json::json!({"installationId":"aggregate","artifactId":"steam-game","namespace":"steam","itemId":"100","complete":true,"dir":payload})
        );
        write(&profile.join("installations.jsonl"), &journal);
        write(
            &profile.join("cache/prod/catalog-cache.json"),
            r#"{"items":{"catalog-id":{"item":{"title":"Preview fixture","productSlug":"fixture","keyImages":[{"type":"DieselGameBoxTall","url":"https://example.com/cover.jpg"}]}}}}"#,
        );
        let report = report();
        let mut scan = fixture(&report, dir.path());
        epic_beta::scan(&mut scan);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.games.len(), 1);
        let game = &scan.games[0];
        assert_eq!(game.title, "Preview fixture");
        assert_eq!(game.installations[0].state, "installed");
        assert!(game.launch.is_some());
        assert!(
            matches!(game.store_page.as_ref().unwrap().target, ActionTarget::Url { ref url } if url.ends_with("/p/fixture"))
        );
        write(&profile.join("installations.jsonl"), &(journal + "{broken"));
        let mut scan = fixture(&report, dir.path());
        epic_beta::scan(&mut scan);
        assert!(scan.games.is_empty());
        assert_eq!(scan.warnings.len(), 1);
    }
    #[test]
    fn xbox_imports_only_game_entitlements_and_uses_nexus_internal_id() {
        let dir = tempfile::tempdir().unwrap();
        let payload = dir.path().join("game");
        fs::create_dir(&payload).unwrap();
        write(
            &dir.path().join(".local/state/ferestre/library.json"),
            r#"{"items":[{"productId":"9NTESTGAME01","productType":"Game","status":"Active"},{"productId":"9NTESTAPP001","productType":"Application","status":"Active"},{"productId":"9NTESTGAME02","productType":"Game","status":"Revoked"}]}"#,
        );
        let profile = dir.path().join(".config/xodus-nexus");
        write(
            &profile.join("shortcuts/launch-game"),
            "fixture, never executed",
        );
        write(&profile.join("launcher-state.json"), &serde_json::json!({"games":[{"id":"0123456789abcdef","storeId":"9NTESTGAME01","title":"Nexus fixture","path":payload,"installState":"ready"}]}).to_string());
        let report = report();
        let mut scan = fixture(&report, dir.path());
        xbox::scan(&mut scan);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.games.len(), 1);
        let game = &scan.games[0];
        assert_eq!(game.title, "Nexus fixture");
        assert!(game.in_library);
        assert!(
            matches!(game.launch.as_ref().unwrap().target, ActionTarget::Command { ref args, .. } if args == &["0123456789abcdef"])
        );
    }
    #[test]
    fn lost_generic_sources_preserve_foreign_installs_without_resurrecting_removed_core_games() {
        let mut previous = report();
        let dir = tempfile::tempdir().unwrap();
        let native = catalog::install(
            "epic-manifests",
            dir.path().display().to_string(),
            Os::Windows,
            true,
        );
        previous.epic.games.push(EpicGame {
            app_name: "same".into(),
            title: "same".into(),
            library_sources: vec![],
            installations: vec![native.clone()],
        });
        let extra = catalog::install(
            "foreign-beta-journal",
            "/foreign/game".into(),
            Os::Windows,
            true,
        );
        previous.catalog.push(catalog::entry(
            "epic",
            "same",
            "Same game",
            vec![native, extra],
            "epic",
        ));
        previous.catalog[0].client_os.push(Os::Linux);
        let report = report();
        let mut scan = fixture(&report, dir.path());
        retain(&mut scan, &previous);
        assert_eq!(scan.games[0].installations.len(), 1);
        assert_eq!(scan.games[0].installations[0].state, "unavailable");
        assert_eq!(scan.games[0].client_os, vec![Os::Windows]);
        assert!(scan.games[0].launch.is_none());
        scan.games.clear();
        scan.verified.insert("foreign-beta-journal".into());
        retain(&mut scan, &previous);
        assert!(scan.games.is_empty());
    }
    #[test]
    fn lutris_matches_store_identity_and_reads_its_own_database_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".local/share/lutris/pga.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE games (id INTEGER,service TEXT,service_id TEXT,name TEXT,installed INTEGER); INSERT INTO games VALUES (1,'uplay','42','Ubisoft fixture',0),(2,'epic','abc','Epic fixture',0),(3,'gog','1','Excluded',0);").unwrap();
        drop(db);
        let bytes = fs::read(&path).unwrap();
        let report = report();
        let mut scan = fixture(&report, dir.path());
        third_party::scan(&mut scan);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert_eq!(scan.games.len(), 2);
        assert!(scan.games.iter().any(|g| g.key == "ubisoft:42"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

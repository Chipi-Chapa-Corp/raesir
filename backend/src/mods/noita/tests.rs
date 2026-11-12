use super::*;

pub(super) fn game() -> LibraryGame {
    crate::catalog::entry("steam", "881100", "Noita", vec![], "test")
}

pub(super) fn manager(root: &Path) -> Manager {
    let manager = Manager::new(&root.join("data"), &root.join("cache"));
    cache_json(
        &manager,
        "https://api.modworkshop.net/games/noita/mods",
        &serde_json::json!({"data":[],"meta":{"last_page":1}}),
    );
    manager
}

fn cache_json(manager: &Manager, url: &str, value: &Value) {
    crate::storage::write_json(
        &manager
            .cache
            .join("providers")
            .join(format!("{}.json", launch::hash(url.as_bytes()))),
        value,
    )
    .unwrap();
}

fn zip(path: &Path, files: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn cached_download(manager: &Manager, url: &str, files: &[(&str, &[u8])]) {
    let root = manager.cache.join("downloads");
    fs::create_dir_all(&root).unwrap();
    zip(&root.join(launch::hash(url.as_bytes())), files);
}

#[test]
fn nexus_import_and_removal_work_while_noita_or_its_proxy_owns_the_game_lock() {
    let dir = tempfile::tempdir().unwrap();
    let mut manager = manager(dir.path());
    manager.leases = dir.path().join("leases");
    let game_dir = dir.path().join("game");
    fs::create_dir_all(game_dir.join("mods/live")).unwrap();
    fs::write(game_dir.join("mods/live/init.lua"), b"running mod").unwrap();
    let mut game = game();
    game.installations.push(crate::model::Installation {
        path: game_dir.to_string_lossy().into(),
        library: dir.path().to_string_lossy().into(),
        os: crate::model::Os::host(),
        state: "installed".into(),
        last_seen: 0,
    });
    let _lease = manager.game_lease(&game_dir).unwrap();
    manager.active.lock().unwrap().insert(game_dir.clone());
    let archive = dir.path().join("mod.zip");
    zip(
        &archive,
        &[
            ("new/mod.xml", b"<Mod name=\"New\"/>"),
            ("new/init.lua", b"next launch"),
        ],
    );
    manager
        .import_nexus(
            &game,
            "https://www.nexusmods.com/noita/mods/1",
            &archive,
            "",
        )
        .unwrap();
    let installed = manager.own_path("noita").unwrap();
    assert!(installed.join("mods/new/init.lua").is_file());
    manager.uninstall(&game, "nexus:1").unwrap();
    assert!(!manager.own_path("noita").unwrap().join("mods/new").exists());
    assert_eq!(
        fs::read(game_dir.join("mods/live/init.lua")).unwrap(),
        b"running mod"
    );
    assert!(!game_dir.join("mods/new").exists());
}

#[test]
fn nexus_import_requires_a_matching_source_page_and_preserves_other_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    let archive = dir.path().join("mod.zip");
    zip(
        &archive,
        &[
            ("test_mod/mod.xml", b"<Mod name=\"Imported Test\"/>"),
            ("test_mod/init.lua", b"return true"),
        ],
    );
    assert!(
        manager
            .import_nexus(&game(), "https://example.com/noita/mods/1", &archive, "")
            .is_err()
    );
    assert!(
        manager
            .import_nexus(
                &game(),
                "https://www.nexusmods.com/skyrim/mods/1",
                &archive,
                ""
            )
            .is_err()
    );
    let snapshot = manager
        .import_nexus(
            &game(),
            "https://www.nexusmods.com/noita/mods/1",
            &archive,
            "",
        )
        .unwrap();
    assert_eq!(snapshot.profiles[0].installed[0].name, "Imported Test");
    assert_eq!(snapshot.packages.last().unwrap().source, "Nexus Mods");
    let original = manager.own_path("noita").unwrap();
    assert!(original.join("mods/test_mod/init.lua").is_file());
    manager.create_profile(&game(), "Separate").unwrap();
    assert!(manager.profiles("noita")[1].installed.is_empty());
    manager.select(&game(), "exposed-games").unwrap();
    manager.uninstall(&game(), "nexus:1").unwrap();
    assert!(
        !manager
            .own_path("noita")
            .unwrap()
            .join("mods/test_mod")
            .exists()
    );
    assert!(original.join("mods/test_mod/init.lua").is_file());
}

#[test]
fn flat_archives_need_an_explicit_folder_and_conflicts_are_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    let archive = dir.path().join("mod.zip");
    zip(
        &archive,
        &[
            (
                "mod.xml",
                b"<Mod name=\"Flat\" request_no_api_restrictions=\"1\"/>",
            ),
            ("init.lua", b"original"),
        ],
    );
    let url = "https://www.nexusmods.com/games/noita/mods/2";
    assert!(manager.import_nexus(&game(), url, &archive, "").is_err());
    assert!(
        manager
            .import_nexus(&game(), url, &archive, "../escape")
            .is_err()
    );
    manager
        .import_nexus(&game(), url, &archive, "flat")
        .unwrap();
    let root = manager.profile_root("noita", "exposed-games");
    let pointer = fs::read(root.join("current.json")).unwrap();
    assert!(
        manager
            .import_nexus(
                &game(),
                "https://www.nexusmods.com/noita/mods/3",
                &archive,
                "flat"
            )
            .is_err()
    );
    assert_eq!(fs::read(root.join("current.json")).unwrap(), pointer);
    assert_eq!(
        fs::read(
            manager
                .own_path("noita")
                .unwrap()
                .join("mods/flat/init.lua")
        )
        .unwrap(),
        b"original"
    );
}

#[test]
fn archive_traversal_and_symlink_targets_never_escape_staging() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bad.zip");
    let target = dir.path().join("staging");
    zip(&source, &[("../escape", b"not allowed")]);
    assert!(archive::extract(&source, &target).is_err());
    assert!(!dir.path().join("escape").exists());
    assert!(archive::destination(&target, Path::new("C:\\escape")).is_err());
    #[cfg(unix)]
    {
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(dir.path(), target.join("link")).unwrap();
        assert!(archive::destination(&target, Path::new("link/out")).is_err());
    }
}

#[test]
fn bundled_recipe_installs_a_matching_pair_and_a_failed_tool_update_keeps_live_revision() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    let recipe = &recipes::for_game(&game()).unwrap().mods[0];
    let host = if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let tool = &recipe.companions[host];
    let mod_url = "https://example.invalid/mod.zip";
    let tool_url = "https://example.invalid/proxy.zip";
    cache_json(
        &manager,
        &format!(
            "https://api.github.com/repos/{}/releases/latest",
            recipe.repository
        ),
        &serde_json::json!({
            "tag_name":"v-test", "assets":[{"name":recipe.mod_asset,"browser_download_url":mod_url},{"name":tool.asset,"browser_download_url":tool_url}]
        }),
    );
    cached_download(
        &manager,
        mod_url,
        &[
            ("mod.xml", b"<Mod name=\"Entangled Worlds\"/>"),
            ("init.lua", b"mod"),
        ],
    );
    cached_download(
        &manager,
        tool_url,
        &[
            (&tool.executable, b"test fixture, never executed"),
            ("libsteam_api.so", b"fixture"),
        ],
    );
    let mut plain = recipe.clone();
    plain.companions.clear();
    assert!(manager.download_recipe(&plain).unwrap().3.is_none());
    manager.install(&game(), "recipe:entangled-worlds").unwrap();
    let current = manager.own_path("noita").unwrap();
    let records = read(&current).unwrap();
    assert_eq!(records[0].package.version, "v-test");
    assert!(current.join("mods/quant.ew/init.lua").is_file());
    assert!(
        current
            .join(&records[0].companion.as_ref().unwrap().executable)
            .is_file()
    );
    let pointer = fs::read(
        manager
            .profile_root("noita", "exposed-games")
            .join("current.json"),
    )
    .unwrap();
    cached_download(
        &manager,
        tool_url,
        &[("wrong-executable", b"invalid replacement")],
    );
    assert!(manager.install(&game(), "recipe:entangled-worlds").is_err());
    assert_eq!(
        fs::read(
            manager
                .profile_root("noita", "exposed-games")
                .join("current.json")
        )
        .unwrap(),
        pointer
    );
    assert_eq!(
        fs::read(current.join("mods/quant.ew/init.lua")).unwrap(),
        b"mod"
    );
}

#[test]
fn modworkshop_selects_the_requested_file_and_rejects_another_game() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    cache_json(
        &manager,
        "https://api.modworkshop.net/mods/12",
        &serde_json::json!({"id":12,"name":"ModWorkshop test","game":{"short_name":"noita"}}),
    );
    cache_json(
        &manager,
        "https://api.modworkshop.net/mods/12/files",
        &serde_json::json!({"data":[
            {"id":51,"name":"first","type":"zip","version":"1","download_url":"https://example.invalid/first.zip"},
            {"id":52,"name":"second","type":"zip","version":"2","download_url":"https://example.invalid/second.zip"}
        ]}),
    );
    cached_download(
        &manager,
        "https://example.invalid/second.zip",
        &[
            ("selected/mod.xml", b"<Mod name=\"Second variant\"/>"),
            ("selected/init.lua", b"selected"),
        ],
    );
    assert_eq!(
        manager.mod_files(&game(), "modworkshop:12").unwrap().len(),
        2
    );
    manager.install(&game(), "modworkshop:12:52").unwrap();
    let current = manager.own_path("noita").unwrap();
    assert_eq!(read(&current).unwrap()[0].package.version, "2");
    assert!(current.join("mods/selected/init.lua").is_file());
    cache_json(
        &manager,
        "https://api.modworkshop.net/mods/13",
        &serde_json::json!({"id":13,"game":{"short_name":"different"}}),
    );
    assert!(
        manager
            .install(&game(), "modworkshop:13")
            .unwrap_err()
            .contains("different game")
    );
}

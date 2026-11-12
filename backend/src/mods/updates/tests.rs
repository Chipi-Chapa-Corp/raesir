use super::*;
use std::io::Write;

fn manager(root: &Path) -> Manager {
    let mut m = Manager::new(&root.join("data"), &root.join("cache"));
    m.leases = root.join("leases");
    m
}

fn index(rows: &[(&str, &str, Vec<&str>)]) -> Arc<PackageIndex> {
    index_url(rows, "https://example.invalid/fixture.zip")
}

fn index_url(rows: &[(&str, &str, Vec<&str>)], url: &str) -> Arc<PackageIndex> {
    let packages=rows.iter().map(|(name,version,deps)|serde_json::from_value::<thunderstore_engine::models::Package>(serde_json::json!({
        "name":name,"full_name":format!("Owner-{name}"),"owner":"Owner","date_created":"2025-01-01T00:00:00Z","date_updated":"2026-01-01T00:00:00Z","categories":[],"versions":[{
            "name":name,"full_name":format!("Owner-{name}-{version}"),"version_number":version,"dependencies":deps,"download_url":url,"date_created":"2026-01-01T00:00:00Z","is_active":true
        }]
    })).unwrap()).collect::<Vec<_>>();
    Arc::new(packages.into())
}

fn cache_archive(manager: &Manager, name: &str, version: &str, payload: &[u8]) {
    let path = manager
        .client("peak")
        .unwrap()
        .cache_dir()
        .join("downloads")
        .join(format!("Owner-{name}-{version}.zip"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(
        format!("{{\"name\":\"{name}\",\"version_number\":\"{version}\",\"dependencies\":[]}} ")
            .as_bytes(),
    )
    .unwrap();
    zip.start_file(
        format!("plugins/{name}.dll"),
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(payload).unwrap();
    zip.finish().unwrap();
}

fn entry(name: &str, version: &str) -> modlist::ProfileMod {
    let v:thunderstore_engine::models::Version=serde_json::from_value(serde_json::json!({"name":name,"version_number":version,"dependencies":[],"date_created":"2025-01-01T00:00:00Z"})).unwrap();
    profile::mod_entry_from_version(&format!("Owner-{name}"), "Owner", &v, 0)
}

fn source(manager: &Manager, slug: &str, id: &str, mods: &[modlist::ProfileMod]) -> PathBuf {
    let path = manager
        .profile_root(slug, id)
        .join("revisions/profile-fixture");
    fs::create_dir_all(&path).unwrap();
    modlist::write(&path, mods).unwrap();
    crate::storage::write_json(
        &manager.profile_root(slug, id).join("current.json"),
        &serde_json::json!({"revision":"profile-fixture"}),
    )
    .unwrap();
    path
}

#[test]
fn installs_and_uninstalls_while_running_keep_the_session_revision_across_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let other = manager(temp.path());
    let game_dir = temp.path().join("game");
    fs::create_dir_all(&game_dir).unwrap();
    fs::write(game_dir.join("winhttp.dll"), b"running loader").unwrap();
    let game = crate::catalog::entry(
        "steam",
        "3527290",
        "Peak",
        vec![crate::model::Installation {
            path: game_dir.to_string_lossy().into(),
            library: temp.path().to_string_lossy().into(),
            os: crate::model::Os::host(),
            state: "installed".into(),
            last_seen: 0,
        }],
        "fixture",
    );
    let original = source(&m, "peak", "exposed-games", &[entry("Root", "1.0.0")]);
    fs::write(original.join("running.cfg"), b"session settings").unwrap();
    let _game_lease = other.game_lease(&game_dir).unwrap();
    m.active.lock().unwrap().insert(game_dir.clone());
    let session = m.revision_lease(&original).unwrap();
    let second_session = other.revision_lease(&original).unwrap();
    let latest = index(&[
        ("Root", "1.0.0", vec![]),
        ("New", "1.0.0", vec!["Owner-Dependency-1.0.0"]),
        ("Dependency", "1.0.0", vec![]),
    ]);
    m.catalogs.lock().unwrap().insert(
        "peak".into(),
        Catalog {
            updated: Instant::now(),
            index: latest,
            packages: Arc::new(Vec::new()),
        },
    );
    cache_archive(&m, "New", "1.0.0", b"new mod");
    cache_archive(&m, "Dependency", "1.0.0", b"new dependency");
    for _ in 0..3 {
        m.install(&game, "Owner-New").unwrap();
        let next = m.own_path("peak").unwrap();
        assert_ne!(next, original);
        assert_eq!(
            fs::read(next.join("BepInEx/plugins/Owner-New/New.dll")).unwrap(),
            b"new mod"
        );
        m.uninstall(&game, "Owner-New").unwrap();
        let next = m.own_path("peak").unwrap();
        assert!(!next.join("BepInEx/plugins/Owner-New").exists());
        assert!(!next.join("BepInEx/plugins/Owner-Dependency").exists());
    }
    assert_eq!(modlist::read(&original).unwrap().len(), 1);
    assert_eq!(
        fs::read(original.join("running.cfg")).unwrap(),
        b"session settings"
    );
    assert_eq!(
        fs::read(game_dir.join("winhttp.dll")).unwrap(),
        b"running loader"
    );
    drop(session);
    other.prune_revisions("peak");
    assert!(
        original.is_dir(),
        "another session still retains the revision"
    );
    drop(second_session);
    other.prune_revisions("peak");
    assert!(
        !original.exists(),
        "unused old revisions can now be removed"
    );
}

#[test]
fn updates_all_owned_profiles_atomically_keeps_selection_disabled_mods_and_install_reasons() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    m.create_profile(&game, "Other").unwrap();
    let other = m.selected_id("peak");
    let mut old = entry("Root", "1.0.0");
    old.enabled = false;
    let original = source(&m, "peak", "exposed-games", &[old.clone()]);
    let other_path = source(&m, "peak", &other, &[old]);
    let latest = index(&[
        ("Root", "2.0.0", vec!["Owner-Dependency-1.0.0"]),
        ("Dependency", "1.0.0", vec![]),
    ]);
    m.catalogs.lock().unwrap().insert(
        "peak".into(),
        Catalog {
            updated: Instant::now(),
            index: latest,
            packages: Arc::new(Vec::new()),
        },
    );
    cache_archive(&m, "Root", "2.0.0", b"new root");
    cache_archive(&m, "Dependency", "1.0.0", b"new dependency");
    let mut refreshed = HashSet::from(["thunderstore:peak".into()]);
    for (id, path) in [("exposed-games", &original), (other.as_str(), &other_path)] {
        let (updated, manual) = m
            .update_profile(&game, id, path, &mut refreshed, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(updated.len(), 2);
        assert!(manual.is_empty());
        let current = m.own_profile_path("peak", id).unwrap();
        assert_ne!(&current, path);
        let records = modlist::read(&current).unwrap();
        assert!(
            !records
                .iter()
                .find(|r| r.name == "Owner-Root")
                .unwrap()
                .enabled
        );
        assert_eq!(
            records
                .iter()
                .find(|r| r.name == "Owner-Root")
                .unwrap()
                .version_number
                .to_string(),
            "2.0.0"
        );
        let tracked = dependencies::Reasons::read(&current)
            .unwrap()
            .installed(records);
        assert!(
            !tracked
                .iter()
                .find(|r| r.id == "Owner-Root")
                .unwrap()
                .automatic
        );
        assert!(
            tracked
                .iter()
                .find(|r| r.id == "Owner-Dependency")
                .unwrap()
                .automatic
        );
        assert_eq!(
            modlist::read(path).unwrap()[0].version_number.to_string(),
            "1.0.0"
        );
    }
    assert_eq!(m.selected_id("peak"), other);
}

#[test]
fn current_mods_are_not_copied_even_while_the_game_is_locked() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let mut game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let original = source(&m, "peak", "exposed-games", &[entry("Root", "2.0.0")]);
    let latest = index(&[("Root", "2.0.0", vec![])]);
    m.catalogs.lock().unwrap().insert(
        "peak".into(),
        Catalog {
            updated: Instant::now(),
            index: latest,
            packages: Arc::new(Vec::new()),
        },
    );
    let mut refreshed = HashSet::from(["thunderstore:peak".into()]);
    let result = m
        .update_profile(
            &game,
            "exposed-games",
            &original,
            &mut refreshed,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(result.0.is_empty());
    assert_eq!(fs::read_dir(original.parent().unwrap()).unwrap().count(), 1);
    let dir = temp.path().join("game");
    fs::create_dir_all(&dir).unwrap();
    game.installations.push(crate::model::Installation {
        path: dir.to_string_lossy().into(),
        library: temp.path().to_string_lossy().into(),
        os: crate::model::Os::host(),
        state: "installed".into(),
        last_seen: 0,
    });
    let lease = m.game_lease(&dir).unwrap();
    assert!(
        m.update_profile(
            &game,
            "exposed-games",
            &original,
            &mut refreshed,
            &AtomicBool::new(false)
        )
        .unwrap()
        .0
        .is_empty()
    );
    drop(lease);
    assert_eq!(m.own_path("peak").unwrap(), original);
    let before = tree_stamp(&original).unwrap();
    fs::write(original.join("player.cfg"), b"changed").unwrap();
    assert_ne!(tree_stamp(&original).unwrap(), before);
}

#[test]
fn deferred_updates_are_retried_without_showing_a_mod_error_banner() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let mut game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    source(&m, "peak", "exposed-games", &[entry("Root", "1.0.0")]);
    let game_dir = temp.path().join("game");
    fs::create_dir_all(&game_dir).unwrap();
    game.installations.push(crate::model::Installation {
        path: game_dir.to_string_lossy().into(),
        library: temp.path().to_string_lossy().into(),
        os: crate::model::Os::host(),
        state: "installed".into(),
        last_seen: 0,
    });
    let _lease = m.profile_lease("peak").unwrap();
    let report = m
        .auto_update(&[game.clone()], true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(report.issues[0].kind, "deferred");
    assert!(m.update_warnings(&game).is_empty());
    let state: State =
        serde_json::from_slice(&fs::read(m.data.join("update-status.json")).unwrap()).unwrap();
    assert!(state.profiles["peak/exposed-games"].next_check <= report.checked_at + 60);
}

#[test]
fn planners_never_downgrade_or_replace_local_mods_and_detect_dependency_updates() {
    let latest = index(&[
        ("Root", "1.0.0", vec![]),
        ("Local", "2.0.0", vec![]),
        ("Dependency", "2.0.0", vec![]),
    ]);
    let mut local = entry("Local", "1.0.0");
    local.online_source = false;
    assert_eq!(
        thunderstore_updates(
            &[entry("Root", "2.0.0"), local, entry("Dependency", "1.0.0")],
            &latest
        ),
        ["Owner-Dependency"]
    );
}

#[test]
fn hourly_schedule_persists_manual_sources_and_catalog_missing_games_without_touching_selection() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "881100", "Noita", vec![], "fixture");
    let root = m.profile_root("noita", "exposed-games");
    let path = root.join("revisions/profile-fixture");
    fs::create_dir_all(&path).unwrap();
    crate::storage::write_json(
        &root.join("current.json"),
        &serde_json::json!({"revision":"profile-fixture"}),
    )
    .unwrap();
    crate::storage::write_json(&path.join("noita-mods.json"),&serde_json::json!([{"package":{"id":"nexus:12","name":"Imported","owner":"","description":"","version":"Imported","page":"https://www.nexusmods.com/noita/mods/12","icon":"","downloads":0,"deprecated":false,"categories":[],"source":"Nexus Mods"},"folders":[],"unsafe_mod":false,"companion":null}])).unwrap();
    let result = m.auto_update(&[], true, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.checked_profiles, 1);
    assert_eq!(result.issues[0].kind, "manual");
    assert!(result.updated.is_empty());
    let again = m
        .auto_update(&[game.clone()], false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(again.checked_profiles, 0);
    assert!(!m.update_warnings(&game).is_empty());
    assert_eq!(m.own_path("noita").unwrap(), path);
    let restarting = m
        .auto_update(&[game], true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(restarting.checked_profiles, 1);
    let contents = fs::read(m.data.join("update-status.json")).unwrap();
    assert!(
        m.auto_update(&[], true, &AtomicBool::new(true))
            .unwrap()
            .updated
            .is_empty()
    );
    assert_eq!(
        fs::read(m.data.join("update-status.json")).unwrap(),
        contents
    );
}

#[test]
fn game_start_during_download_allows_updates_while_profile_edits_defer_publication() {
    use std::{
        io::{Cursor, Read},
        net::TcpListener,
        sync::mpsc,
    };
    for player_edits in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let m = manager(temp.path());
        let dir = temp.path().join("game");
        fs::create_dir_all(&dir).unwrap();
        let game = crate::catalog::entry(
            "steam",
            "3527290",
            "Peak",
            vec![crate::model::Installation {
                path: dir.to_string_lossy().into(),
                library: temp.path().to_string_lossy().into(),
                os: crate::model::Os::host(),
                state: "installed".into(),
                last_seen: 0,
            }],
            "fixture",
        );
        let original = source(&m, "peak", "exposed-games", &[entry("Root", "1.0.0")]);
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let latest = index_url(
            &[("Root", "2.0.0", vec![])],
            &format!("http://{address}/mod.zip"),
        );
        m.catalogs.lock().unwrap().insert(
            "peak".into(),
            Catalog {
                updated: Instant::now(),
                index: latest,
                packages: Arc::new(Vec::new()),
            },
        );
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{\"version_number\":\"2.0.0\"}").unwrap();
        zip.start_file("plugins/Root.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"new version").unwrap();
        let zip = zip.finish().unwrap().into_inner();
        let (started, receive) = mpsc::channel();
        let (resume, wait) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let (mut stream, _) = socket.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                started.send(()).unwrap();
                let _ = wait.recv_timeout(Duration::from_secs(10));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    zip.len()
                )
                .unwrap();
                stream.write_all(&zip).unwrap();
            });
            let job = scope.spawn(|| {
                m.update_profile(
                    &game,
                    "exposed-games",
                    &original,
                    &mut HashSet::from(["thunderstore:peak".into()]),
                    &AtomicBool::new(false),
                )
            });
            receive.recv_timeout(Duration::from_secs(10)).unwrap();
            {
                let _guard = m
                    .operation
                    .try_lock()
                    .expect("downloads must not block Play");
            }
            let lease = m
                .game_lease(&dir)
                .expect("no game lease held during download");
            if player_edits {
                drop(lease);
                fs::write(original.join("player.cfg"), b"keep my edit").unwrap();
                resume.send(()).unwrap();
            } else {
                resume.send(()).unwrap();
                let (updated, _) = job.join().unwrap().unwrap();
                assert_eq!(updated, ["Owner-Root"]);
                let current = m.own_path("peak").unwrap();
                assert_ne!(current, original);
                assert_eq!(
                    fs::read(current.join("BepInEx/plugins/Owner-Root/Root.dll")).unwrap(),
                    b"new version"
                );
                drop(lease);
                return;
            }
            let error = job.join().unwrap().unwrap_err();
            assert!(error.starts_with("Deferred:"), "{error}");
        });
        if player_edits {
            assert_eq!(m.own_path("peak").unwrap(), original);
        }
        assert_eq!(
            modlist::read(&original).unwrap()[0]
                .version_number
                .to_string(),
            "1.0.0"
        );
        if player_edits {
            assert_eq!(
                fs::read(original.join("player.cfg")).unwrap(),
                b"keep my edit"
            );
        }
    }
}

#[test]
fn broken_archives_never_publish_a_partially_updated_profile() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let original = source(&m, "peak", "exposed-games", &[entry("Root", "1.0.0")]);
    let latest = index(&[("Root", "2.0.0", vec![])]);
    m.catalogs.lock().unwrap().insert(
        "peak".into(),
        Catalog {
            updated: Instant::now(),
            index: latest,
            packages: Arc::new(Vec::new()),
        },
    );
    let downloads = m.client("peak").unwrap().cache_dir().join("downloads");
    fs::create_dir_all(&downloads).unwrap();
    fs::write(downloads.join("Owner-Root-2.0.0.zip"), b"incomplete zip").unwrap();
    assert!(
        m.update_profile(
            &game,
            "exposed-games",
            &original,
            &mut HashSet::from(["thunderstore:peak".into()]),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(m.own_path("peak").unwrap(), original);
    assert_eq!(
        modlist::read(&original).unwrap()[0]
            .version_number
            .to_string(),
        "1.0.0"
    );
}

#[test]
fn cross_process_profile_mutations_are_protected_even_when_the_game_is_uninstalled() {
    let temp = tempfile::tempdir().unwrap();
    let first = manager(temp.path());
    let second = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let original = source(&first, "peak", "exposed-games", &[entry("Root", "1.0.0")]);
    let busy = first.profile_lease("peak").unwrap();
    assert!(
        second
            .update_profile(
                &game,
                "exposed-games",
                &original,
                &mut HashSet::new(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .starts_with("Deferred:")
    );
    assert!(second.try_profile_lease("peak").is_err());
    assert_eq!(first.own_path("peak").unwrap(), original);
    std::thread::scope(|scope| {
        let (ready, waiting) = std::sync::mpsc::channel();
        let (done, result) = std::sync::mpsc::channel();
        scope.spawn(move || {
            ready.send(()).unwrap();
            done.send(second.select(&game, "exposed-games")).unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        // User operations wait their turn instead of failing on a busy profile.
        let early = result.recv_timeout(Duration::from_millis(50));
        drop(busy);
        assert!(matches!(
            early,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
    });
}

#[test]
fn automatic_dependency_updates_cannot_overwrite_a_local_mod() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let mut local = entry("Local", "1.0.0");
    local.online_source = false;
    let original = source(
        &m,
        "peak",
        "exposed-games",
        &[entry("Root", "1.0.0"), local],
    );
    let latest = index(&[
        ("Root", "2.0.0", vec!["Owner-Local-2.0.0"]),
        ("Local", "2.0.0", vec![]),
    ]);
    m.catalogs.lock().unwrap().insert(
        "peak".into(),
        Catalog {
            updated: Instant::now(),
            index: latest,
            packages: Arc::new(Vec::new()),
        },
    );
    let error = m
        .update_profile(
            &game,
            "exposed-games",
            &original,
            &mut HashSet::from(["thunderstore:peak".into()]),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(error.contains("replace local mod"), "{error}");
    assert_eq!(m.own_path("peak").unwrap(), original);
    assert!(!modlist::read(&original).unwrap()[1].online_source);
}

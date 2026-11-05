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
fn updates_all_owned_profiles_atomically_keeps_selection_disabled_mods_and_install_reasons() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    m.create_profile(&game, "Other").unwrap();
    let other = m.selected_id("peak");
    let mut old = entry("Root", "1.0.0");
    old.enabled = false;
    let original = source(&m, "peak", "raesir", &[old.clone()]);
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
    for (id, path) in [("raesir", &original), (other.as_str(), &other_path)] {
        let (updated, manual) = m
            .update_profile(
                &game,
                &[],
                id,
                path,
                &mut refreshed,
                &AtomicBool::new(false),
            )
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
fn current_mods_are_not_copied_and_offline_or_busy_profiles_keep_their_current_pointer() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let mut game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let original = source(&m, "peak", "raesir", &[entry("Root", "2.0.0")]);
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
            &[],
            "raesir",
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
            &[],
            "raesir",
            &original,
            &mut refreshed,
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .starts_with("Deferred:")
    );
    drop(lease);
    assert_eq!(m.own_path("peak").unwrap(), original);
    let before = tree_stamp(&original).unwrap();
    fs::write(original.join("player.cfg"), b"changed").unwrap();
    assert_ne!(tree_stamp(&original).unwrap(), before);
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
    let root = m.profile_root("noita", "raesir");
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
fn a_game_starting_or_profile_edit_during_download_defers_publication_without_holding_the_operation_lock()
 {
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
        let original = source(&m, "peak", "raesir", &[entry("Root", "1.0.0")]);
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
                    &[],
                    "raesir",
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
                let error = job.join().unwrap().unwrap_err();
                assert!(error.starts_with("Deferred:"), "{error}");
                drop(lease);
                return;
            }
            let error = job.join().unwrap().unwrap_err();
            assert!(error.starts_with("Deferred:"), "{error}");
        });
        assert_eq!(m.own_path("peak").unwrap(), original);
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
    let original = source(&m, "peak", "raesir", &[entry("Root", "1.0.0")]);
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
            &[],
            "raesir",
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
    let original = source(&first, "peak", "raesir", &[entry("Root", "1.0.0")]);
    let busy = first.profile_lease("peak").unwrap();
    assert!(
        second
            .update_profile(
                &game,
                &[],
                "raesir",
                &original,
                &mut HashSet::new(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .starts_with("Deferred:")
    );
    assert!(second.select(&game, "raesir").is_err());
    assert_eq!(first.own_path("peak").unwrap(), original);
    drop(busy);
    assert!(second.select(&game, "raesir").is_ok());
}

#[test]
fn automatic_dependency_updates_cannot_overwrite_a_local_mod() {
    let temp = tempfile::tempdir().unwrap();
    let m = manager(temp.path());
    let game = crate::catalog::entry("steam", "3527290", "Peak", vec![], "fixture");
    let mut local = entry("Local", "1.0.0");
    local.online_source = false;
    let original = source(&m, "peak", "raesir", &[entry("Root", "1.0.0"), local]);
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
            &[],
            "raesir",
            &original,
            &mut HashSet::from(["thunderstore:peak".into()]),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(error.contains("replace local mod"), "{error}");
    assert_eq!(m.own_path("peak").unwrap(), original);
    assert!(!modlist::read(&original).unwrap()[1].online_source);
}

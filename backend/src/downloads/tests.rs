use super::*;

#[test]
fn transfer_counters_are_bounded_and_never_claim_commit_from_percentage_alone() {
    assert_eq!(
        parse_progress("[DLManager] INFO: = Progress: 37.53% (3/8), ETA: 01:20"),
        Some(37)
    );
    assert_eq!(parse_progress("Progress: 100.00% (8/8)"), Some(99));
    assert_eq!(
        parse_progress(r#"{"progress":{"done":25,"total":100}}"#),
        Some(25)
    );
    for line in [
        "Progress: NaN%",
        "Progress: 150%",
        "Progress: -3%",
        "warning 52%",
        r#"{"progress":{"done":1,"total":0}}"#,
    ] {
        assert_eq!(parse_progress(line), None);
    }
}

#[test]
fn epic_update_plan_matches_exact_app_profile_platform_and_version() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("legendary");
    let payload = temp.path().join("game");
    fs::create_dir_all(&payload).unwrap();
    storage::write_json(&profile.join("installed.json"),&serde_json::json!({"exact-id":{"version":"1","platform":"Windows","install_path":payload}})).unwrap();
    storage::write_json(
        &profile.join("assets.json"),
        &serde_json::json!({"Windows":[{"app_name":"exact-id","build_version":"2"}]}),
    )
    .unwrap();
    let mut game = crate::catalog::entry(
        "epic",
        "exact-id",
        "Fixture",
        vec![crate::catalog::install(
            &profile.to_string_lossy(),
            payload.to_string_lossy().into_owned(),
            Os::host(),
            true,
        )],
        "fixture",
    );
    game.in_library = true;
    let mut report:Report=serde_json::from_value(serde_json::json!({"host_os":Os::host(),"games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null})).unwrap();
    report.epic.sources.push(EpicSource {
        path: profile.to_string_lossy().into_owned(),
        kind: "legendary".into(),
        os: Os::host(),
        local: true,
        drive_c: None,
    });
    let request =
        legendary_plan(&report, &game, Operation::Update, "fixture-helper".into()).unwrap();
    assert_eq!(request.version, "2");
    assert_eq!(request.args.last().unwrap(), "exact-id");
    assert_eq!(
        request.env["LEGENDARY_CONFIG_PATH"],
        profile.to_string_lossy()
    );
    assert!(!request.completion.installed("2"));
    assert!(request.completion.installed("1"));
    let mut other = game.clone();
    other.id = "same-title-other-id".into();
    assert!(legendary_plan(&report, &other, Operation::Update, "fixture-helper".into()).is_none());
    report.epic.sources[0].local = false;
    assert!(legendary_plan(&report, &game, Operation::Update, "fixture-helper".into()).is_none());
}

#[test]
fn an_update_finished_in_the_owning_client_stops_blocking_play() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Manager::new(temp.path());
    let profile = temp.path().join("profile");
    let payload = temp.path().join("game");
    fs::create_dir_all(&payload).unwrap();
    let write_version = |version| {
        storage::write_json(
            &profile.join("installed.json"),
            &serde_json::json!({"fixture":{"version":version,"install_path":payload}}),
        )
        .unwrap()
    };
    write_version("1");
    let request = Request {
        key: "epic:fixture".into(),
        operation: Operation::Update,
        program: "must-not-run".into(),
        args: vec![],
        env: BTreeMap::new(),
        completion: Completion::Legendary {
            profile: profile.clone(),
            app: "fixture".into(),
        },
        version: "2".into(),
        game_dir: None,
        previous_version: None,
        game_lock: None,
    };
    manager.available(request.clone()).unwrap();
    assert!(manager.snapshot()[&request.key].blocks_play());
    write_version("2");
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Complete);
    assert!(!manager.snapshot()[&request.key].blocks_play());
}

#[cfg(unix)]
#[test]
fn worker_progress_pause_resume_adoption_and_committed_version_use_one_job() {
    worker_flow(false, false);
}

#[cfg(unix)]
#[test]
fn flatpak_host_worker_uses_the_same_progress_pause_resume_and_commit_contract() {
    worker_flow(true, false);
}

#[cfg(unix)]
#[test]
fn a_client_refreshing_a_queued_manifest_commits_the_newer_version() {
    worker_flow(false, true);
}

#[cfg(unix)]
#[test]
fn the_flatpak_worker_accepts_a_confirmed_newer_manifest_too() {
    worker_flow(true, true);
}

#[cfg(unix)]
fn worker_flow(host: bool, refreshed: bool) {
    let temp = tempfile::tempdir().unwrap();
    let manager = Manager::new(temp.path());
    let profile = temp.path().join("profile");
    let payload = temp.path().join("game");
    fs::create_dir_all(&payload).unwrap();
    fs::create_dir_all(&profile).unwrap();
    storage::write_json(
        &profile.join("installed.json"),
        &serde_json::json!({"fixture":{"version":"1","install_path":payload}}),
    )
    .unwrap();
    let committed_version = if refreshed { "3" } else { "2" };
    let script = temp.path().join("client.py");
    fs::write(&script,r#"import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]); profile=Path(sys.argv[2]); payload=Path(sys.argv[3])
print('Progress: 17.50% (1/8)',flush=True)
deadline=time.monotonic()+20
while not (root/'finish').exists() and time.monotonic()<deadline:
    (root/'heartbeat').write_text(str(time.monotonic()))
    time.sleep(.03)
profile.mkdir(exist_ok=True)
(profile/'installed.json').write_text(json.dumps({'fixture':{'version':sys.argv[4],'install_path':str(payload)}}))
(profile/'assets.json').write_text(json.dumps({'Windows':[{'app_name':'fixture','build_version':sys.argv[4]}]}))
print('Progress: 100.00% (8/8)',flush=True)
"#).unwrap();
    let request = Request {
        key: "epic:fixture".into(),
        operation: Operation::Update,
        program: "python3".into(),
        args: vec![
            script.to_string_lossy().into_owned(),
            temp.path().to_string_lossy().into_owned(),
            profile.to_string_lossy().into_owned(),
            payload.to_string_lossy().into_owned(),
            committed_version.into(),
        ],
        env: BTreeMap::new(),
        completion: Completion::Legendary {
            profile,
            app: "fixture".into(),
        },
        version: "2".into(),
        game_dir: None,
        previous_version: None,
        game_lock: None,
    };
    manager.available(request.clone()).unwrap();
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Available);
    let dir = manager.directory(&request.key);
    let thread_dir = dir.clone();
    let worker = std::thread::spawn(move || {
        if host {
            let mut command = Command::new("python3");
            command
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/downloads/host_worker.py"))
                .arg(thread_dir);
            assert!(command.status().unwrap().success());
        } else {
            worker::run(&thread_dir)
        }
    });
    struct Cleanup(PathBuf, PathBuf, Option<std::thread::JoinHandle<()>>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = storage::write_json(
                &self.1.join("control.json"),
                &serde_json::json!({"resume":true,"generation":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()}),
            );
            let _ = fs::write(self.0.join("finish"), b"done");
            if let Some(t) = self.2.take() {
                let _ = t.join();
            }
        }
    }
    let cleanup = Cleanup(temp.path().into(), dir, Some(worker));
    let wait = |phase: Phase, percent: Option<u8>| {
        let started = Instant::now();
        loop {
            if let Some(p) = manager.snapshot().get(&request.key)
                && p.phase == phase
                && (percent.is_none() || p.percent == percent)
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "download never reached {phase:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    wait(Phase::Downloading, Some(17));
    manager.control(&request.key, false).unwrap();
    wait(Phase::Paused, None);
    std::thread::sleep(Duration::from_millis(100));
    let heartbeat = fs::read(temp.path().join("heartbeat")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(fs::read(temp.path().join("heartbeat")).unwrap(), heartbeat);
    let reopened = Manager::new(temp.path());
    assert_eq!(reopened.snapshot()[&request.key].phase, Phase::Paused);
    reopened.control(&request.key, true).unwrap();
    wait(Phase::Downloading, None);
    fs::write(temp.path().join("finish"), b"done").unwrap();
    wait(Phase::Complete, Some(100));
    assert!(request.completion.installed(committed_version));
    manager.finish();
    assert!(manager.snapshot().is_empty());
    drop(cleanup);
}

#[test]
fn a_mod_or_game_lease_prevents_an_update_worker_from_starting() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Manager::new(temp.path());
    let lock_path = temp.path().join("game.lock");
    let lock = fs::File::create(&lock_path).unwrap();
    lock.try_lock().unwrap();
    let request = Request {
        key: "epic:locked".into(),
        operation: Operation::Update,
        program: "must-not-run".into(),
        args: vec![],
        env: BTreeMap::new(),
        completion: Completion::Legendary {
            profile: temp.path().into(),
            app: "locked".into(),
        },
        version: "2".into(),
        game_dir: None,
        previous_version: None,
        game_lock: Some(lock_path),
    };
    manager.available(request.clone()).unwrap();
    worker::run(&manager.directory(&request.key));
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Available);
    assert!(
        manager.snapshot()[&request.key]
            .detail
            .as_ref()
            .unwrap()
            .contains("busy")
    );
}

#[test]
fn a_busy_client_queues_other_games_and_queued_downloads_can_be_paused() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Manager::new(temp.path());
    let profile = temp.path().join("profile");
    fs::create_dir_all(&profile).unwrap();
    let lock = fs::File::create(profile.join("installed.json.lock")).unwrap();
    lock.try_lock().unwrap();
    let request = Request {
        key: "epic:queued".into(),
        operation: Operation::Update,
        program: "must-not-run".into(),
        args: vec![],
        env: BTreeMap::new(),
        completion: Completion::Legendary {
            profile,
            app: "queued".into(),
        },
        version: "2".into(),
        game_dir: None,
        previous_version: None,
        game_lock: None,
    };
    manager.start(request.clone()).unwrap();
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Queued);
    manager.advance_queue();
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Queued);
    manager.control(&request.key, false).unwrap();
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Paused);
    manager.advance_queue();
    assert_eq!(manager.snapshot()[&request.key].phase, Phase::Paused);
}

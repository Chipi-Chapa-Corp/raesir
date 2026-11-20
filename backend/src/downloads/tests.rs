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
fn download_failures_are_visible_until_dismissed_or_restarted() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Manager::new(temp.path());
    let key = "xbox:9NRLQNVKCGWC";
    let request = Request {
        key: key.into(),
        operation: Operation::Install,
        program: "must-not-run".into(),
        args: vec![],
        env: Default::default(),
        completion: Completion::Ferestre {
            record: temp.path().join("installed.json"),
        },
        version: String::new(),
        previous_version: None,
        game_dir: None,
        game_lock: None,
    };
    manager.available(request).unwrap();
    let path = manager.directory(key).join("progress.json");
    let mut progress = Progress::starting(Operation::Install);
    progress.phase = Phase::Failed;
    progress.detail = Some(crate::ferestre::CONSOLE_ONLY_MESSAGE.into());
    storage::write_json(&path, &progress).unwrap();
    assert!(!manager.snapshot().contains_key(key));

    progress.detail = Some("Xbox Store could not complete the download.".into());
    storage::write_json(&path, &progress).unwrap();
    assert_eq!(manager.snapshot().get(key), Some(&progress));

    let seen = HashMap::from([(key.into(), progress.clone())]);
    let mut resumed = progress.clone();
    resumed.phase = Phase::Paused;
    storage::write_json(&path, &resumed).unwrap();
    manager.dismiss_failed(&seen);
    assert_eq!(manager.snapshot().get(key), Some(&resumed));

    storage::write_json(&path, &progress).unwrap();
    let log = path.with_file_name("output.log");
    fs::write(&log, "diagnostics").unwrap();
    manager.dismiss_failed(&seen);
    assert!(!path.exists());
    assert!(log.is_file());

    storage::write_json(&path, &progress).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(60))
        .unwrap();
    let reopened = Manager::new(temp.path());
    assert!(!reopened.snapshot().contains_key(key));
    assert!(!path.exists());
    assert!(log.is_file());

    storage::write_json(&path, &resumed).unwrap();
    let reopened = Manager::new(temp.path());
    assert_eq!(reopened.snapshot().get(key), Some(&resumed));
}

#[cfg(unix)]
#[test]
fn ferestre_workers_show_bytes_and_pause_the_downloader_without_a_terminal() {
    for host in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let manager = Manager::new(temp.path());
        let program = temp.path().join("client.py");
        let record = temp.path().join("installed.json");
        fs::write(&program, r#"import json, os, select, sys
from pathlib import Path
assert os.getsid(0) == os.getpid()
assert os.environ['XODUS_PROGRESS'] == 'json'
assert os.environ['FERESTRE_GUI_CANCELLATION'] == '1'
def report(paused):
    print('FERESTRE_PROGRESS ' + json.dumps(dict(schema=1, phase='downloading',
          completedBytes=16*1024*1024, totalBytes=64*1024*1024, paused=paused, canPause=True)), flush=True)
def control():
    assert select.select([sys.stdin], [], [], 5)[0]
    return sys.stdin.buffer.read(1)
report(False)
assert control() == b'p'
report(True)
assert control() == b'r'
record = Path(sys.argv[1])
(record.parent/'.xodus-streaming.msixvc').touch()
record.write_text(json.dumps(dict(dir=str(record.parent))))
"#).unwrap();
        let key = "xbox:9NTESTGAME01";
        manager
            .available(Request {
                key: key.into(),
                operation: Operation::Install,
                program: "python3".into(),
                args: vec![
                    program.to_string_lossy().into(),
                    record.to_string_lossy().into(),
                ],
                env: Default::default(),
                completion: Completion::Ferestre { record },
                version: String::new(),
                previous_version: None,
                game_dir: None,
                game_lock: None,
            })
            .unwrap();
        let dir = manager.directory(key);
        let worker = std::thread::spawn(move || {
            if host {
                assert!(
                    Command::new("python3")
                        .arg(
                            Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("src/downloads/host_worker.py")
                        )
                        .arg(dir)
                        .status()
                        .unwrap()
                        .success()
                );
            } else {
                worker::run(&dir);
            }
        });
        let wait = |phase| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let progress = manager.snapshot().remove(key).unwrap();
                if progress.phase == phase {
                    return progress;
                }
                assert!(
                    Instant::now() < deadline,
                    "expected {phase:?}, got {progress:?}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        };
        let downloading = wait(Phase::Downloading);
        assert!(downloading.controllable);
        assert_eq!(downloading.percent, Some(25));
        assert!(downloading.label().contains("16.0 of 64.0 MiB"));
        manager.control(key, false).unwrap();
        assert_eq!(wait(Phase::Paused).label(), "Paused");
        manager.control(key, true).unwrap();
        assert_eq!(wait(Phase::Complete).percent, Some(100));
        worker.join().unwrap();
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
#[test]
fn runtime_progress_and_completion_are_readable_after_reopening() {
    for host_worker in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let manager = Manager::new(temp.path());
        let state = temp.path().join("state");
        let cache = temp.path().join("cache");
        let program = temp.path().join("runtime.py");
        fs::write(
            &program,
            r#"import sys,time
from pathlib import Path
root=Path(sys.argv[1])
cache=root/'cache'
cache.mkdir()
archive=cache/'.ferestre-runtime-11.0.0.tar.xz.42-0.part'
with archive.open('wb') as output:
    output.truncate(8*1024*1024)
print('-- downloading runtime 11.0.0 (this is a one-time download)',file=sys.stderr,flush=True)
deadline=time.monotonic()+5
while not (root/'extract').exists() and time.monotonic()<deadline:
    time.sleep(.025)
print('-- unpacking runtime 11.0.0',file=sys.stderr,flush=True)
while not (root/'finish').exists() and time.monotonic()<deadline:
    time.sleep(.025)
runtime=root/'state/runtimes/11.0.0'
dll=runtime/'files/lib/wine/x86_64-windows/xgameruntime.dll'
dll.parent.mkdir(parents=True)
dll.touch()
(runtime/'proton').touch()
"#,
        )
        .unwrap();
        let key = crate::ferestre::RUNTIME_JOB;
        manager
            .available(Request {
                key: key.into(),
                operation: Operation::Install,
                program: "python3".into(),
                args: vec![
                    program.to_string_lossy().into(),
                    temp.path().to_string_lossy().into(),
                ],
                env: Default::default(),
                completion: Completion::FerestreRuntime { state, cache },
                version: String::new(),
                previous_version: None,
                game_dir: None,
                game_lock: None,
            })
            .unwrap();
        let dir = manager.directory(key);
        let worker = std::thread::spawn(move || {
            if host_worker {
                assert!(
                    Command::new("python3")
                        .arg(
                            Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("src/downloads/host_worker.py")
                        )
                        .arg(dir)
                        .status()
                        .unwrap()
                        .success()
                );
            } else {
                worker::run(&dir);
            }
        });
        drop(manager);
        let reopened = Manager::new(temp.path());
        let wait = |phase| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let progress = reopened.snapshot().remove(key).unwrap();
                if progress.phase == phase {
                    return progress;
                }
                assert!(
                    Instant::now() < deadline,
                    "expected {phase:?}, got {progress:?}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        };
        let download = wait(Phase::Downloading);
        assert!(download.label().ends_with("8.0 MiB received"));
        assert!(!download.controllable && download.percent.is_none());
        fs::write(temp.path().join("extract"), b"").unwrap();
        assert!(wait(Phase::Installing).label().starts_with("Extracting"));
        fs::write(temp.path().join("finish"), b"").unwrap();
        assert_eq!(wait(Phase::Complete).percent, Some(100));
        worker.join().unwrap();
    }
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

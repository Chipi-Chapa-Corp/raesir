use crate::{model::OwnedLibrary, platform, steam_control};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, Signal, System};

#[derive(Clone, Debug, Serialize)]
pub struct Client {
    pub root: String,
    pub label: String,
    #[serde(skip)]
    program: PathBuf,
    #[serde(skip)]
    prefix: Vec<String>,
}

impl Client {
    fn command(&self, argument: &str) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.prefix)
            .arg(argument)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }
}

pub fn clients() -> Vec<Client> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for source in platform::local_sources() {
        let Ok(root) = Path::new(&source.path).canonicalize() else {
            continue;
        };
        if !seen.insert(root.clone()) {
            continue;
        }
        let flatpak = source.path.contains("com.valvesoftware.Steam");
        let (program, prefix) = if flatpak {
            let Some(program) = find_program("flatpak") else {
                continue;
            };
            (
                program,
                vec!["run".into(), "com.valvesoftware.Steam".into()],
            )
        } else if cfg!(windows) {
            (root.join("steam.exe"), Vec::new())
        } else if let Some(program) = find_program("steam") {
            (program, Vec::new())
        } else {
            (root.join("steam.sh"), Vec::new())
        };
        if !program.is_file() {
            continue;
        }
        result.push(Client {
            label: format!(
                "{} Steam — {}",
                if flatpak { "Flatpak" } else { "Native" },
                root.display()
            ),
            root: root.to_string_lossy().into(),
            program,
            prefix,
        });
    }
    let running = snapshot();
    result.sort_by_key(|c| {
        (
            !running.processes().values().any(|p| belongs_to(p, &c.root)),
            !c.label.starts_with("Flatpak"),
        )
    });
    result
}

fn find_program(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    pid: Pid,
    started: u64,
}

fn snapshot() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_exe(sysinfo::UpdateKind::OnlyIfNotSet)
            .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
            .without_tasks(),
    );
    system
}

fn is_steam(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case("steam") || name.eq_ignore_ascii_case("steam.exe")
}

fn belongs_to(process: &sysinfo::Process, root: &str) -> bool {
    is_steam(process.name())
        && (process.exe().is_some_and(|exe| exe.starts_with(root))
            || process
                .cmd()
                .first()
                .is_some_and(|arg| Path::new(arg).starts_with(root)))
}

struct Lease {
    client: Client,
    child: Child,
    owned: BTreeSet<Identity>,
    before: BTreeSet<Identity>,
    keep_running: bool,
}

impl Lease {
    fn capture(&mut self, system: &System) {
        let root = Pid::from_u32(self.child.id());
        if let Some(p) = system
            .process(root)
            .filter(|_| self.child.try_wait().ok().flatten().is_none())
        {
            self.owned.insert(Identity {
                pid: root,
                started: p.start_time(),
            });
        }
        let group_is_owned = self.alive(system);
        loop {
            let old_len = self.owned.len();
            for (pid, process) in system.processes() {
                // Record helpers, not arbitrary games that a user might start.
                let helper = process
                    .name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("steam");
                if helper
                    && (process.parent().is_some_and(|parent| {
                        self.owned.iter().any(|p| {
                            p.pid == parent
                                && system
                                    .process(parent)
                                    .is_some_and(|current| current.start_time() == p.started)
                        })
                    }) || (group_is_owned && in_process_group(*pid, root)))
                {
                    let identity = Identity {
                        pid: *pid,
                        started: process.start_time(),
                    };
                    if !self.before.contains(&identity) {
                        self.owned.insert(identity);
                    }
                }
            }
            if self.owned.len() == old_len {
                break;
            }
        }
    }
    fn alive(&self, system: &System) -> bool {
        self.owned.iter().any(|id| {
            system.process(id.pid).is_some_and(|p| {
                p.start_time() == id.started && p.status() != ProcessStatus::Zombie
            })
        })
    }
    fn signal(&self, system: &System, signal: Signal) {
        for id in &self.owned {
            if let Some(p) = system
                .process(id.pid)
                .filter(|p| p.start_time() == id.started)
            {
                let _ = p.kill_with(signal);
            }
        }
    }
    fn stop(&mut self) -> Result<(), String> {
        // A user-requested install/removal belongs to Steam and continues after Raesir
        // closes. Only temporary library-lookup sessions are shut down here.
        if self.keep_running {
            let _ = self.child.try_wait();
            return Ok(());
        }
        let mut system = snapshot();
        self.capture(&system);
        if self.alive(&system) {
            // -shutdown is a global client command: send it only while our own
            // recorded Steam PID still exists. Never send it to a replacement.
            let has_our_steam = self.owned.iter().any(|id| {
                system
                    .process(id.pid)
                    .is_some_and(|p| p.start_time() == id.started && is_steam(p.name()))
            });
            if has_our_steam && let Ok(mut shutdown) = self.client.command("-shutdown").spawn() {
                let until = Instant::now() + Duration::from_secs(3);
                while Instant::now() < until {
                    if shutdown.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if shutdown.try_wait().ok().flatten().is_none() {
                    let _ = shutdown.kill();
                }
                let _ = shutdown.wait();
            }
            let until = Instant::now() + Duration::from_secs(4);
            while Instant::now() < until {
                system = snapshot();
                self.capture(&system);
                if !self.alive(&system) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            self.signal(&system, Signal::Term);
            thread::sleep(Duration::from_millis(150));
            system = snapshot();
            self.signal(&system, Signal::Kill);
        }
        // This is our exact bootstrap Child handle, not an arbitrary Steam PID.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let until = Instant::now() + Duration::from_secs(2);
        while self.alive(&snapshot()) {
            if Instant::now() > until {
                return Err("Steam has not exited. Close Steam, then retry closing Raesir.".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn in_process_group(pid: Pid, group: Pid) -> bool {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_ascii_whitespace().nth(2))
        .and_then(|id| id.parse::<u32>().ok())
        == Some(group.as_u32())
}
#[cfg(not(target_os = "linux"))]
fn in_process_group(_pid: Pid, _group: Pid) -> bool {
    false
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Default)]
pub struct Manager {
    lease: Mutex<Option<Lease>>,
    pub closing: AtomicBool,
    pub cleaned: AtomicBool,
}

#[derive(Serialize)]
pub struct Status {
    pub clients: Vec<Client>,
    pub managed_root: Option<String>,
    pub external_running: bool,
    pub supported: bool,
}

impl Manager {
    /// A mutation delegated through a client URL must survive Raesir closing too.
    pub fn preserve(&self, root: &str) {
        if let Some(lease) = self
            .lease
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            && lease.client.root == root
        {
            lease.keep_running = true;
        }
    }

    pub fn status(&self) -> Status {
        let managed_root = self
            .lease
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|l| l.client.root.clone());
        Status {
            clients: clients(),
            external_running: managed_root.is_none()
                && snapshot().processes().values().any(|p| is_steam(p.name())),
            managed_root,
            supported: crate::steam_ipc::supported(),
        }
    }
    pub fn read_library(&self, root: &str) -> Result<OwnedLibrary, String> {
        if env::var_os("RAESIR_DISABLE_STEAM_IPC").is_some() {
            return Err("Steam IPC is disabled for isolated diagnostics".into());
        }
        if !crate::steam_ipc::supported() {
            return Err("Client-auth lookup requires 64-bit Windows or Linux.".into());
        }
        let client = clients()
            .into_iter()
            .find(|c| c.root == root)
            .ok_or("Choose a locally detected Steam client first.")?;
        self.read_client(client, false, |root| self.probe(root))
    }
    fn read_client<T>(
        &self,
        client: Client,
        keep_running: bool,
        probe: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut lock = self
            .lease
            .lock()
            .map_err(|_| "Steam manager is unavailable")?;
        if self.closing.load(Ordering::SeqCst) {
            return Err("Raesir is shutting down".into());
        }
        if let Some(lease) = lock.as_ref() {
            if lease.client.root != client.root {
                return Err(
                    "Stop the managed Steam client before selecting another installation.".into(),
                );
            }
        } else {
            let before = snapshot();
            if before
                .processes()
                .values()
                .any(|p| belongs_to(p, &client.root))
            {
                // Borrow the running session without creating a lease. Neither
                // Stop nor app exit may send shutdown or signals to this client,
                // even when the lookup fails or the external client exits.
                return probe(&client.root);
            }
            let before = before
                .processes()
                .iter()
                .map(|(pid, p)| Identity {
                    pid: *pid,
                    started: p.start_time(),
                })
                .collect();
            let child = client
                .command("-silent")
                .spawn()
                .map_err(|e| format!("Could not start Steam: {e}"))?;
            *lock = Some(Lease {
                client,
                child,
                owned: BTreeSet::new(),
                before,
                keep_running: false,
            });
            // Track the bootstrap while it starts; no retries that spawn clients.
            for _ in 0..15 {
                lock.as_mut().unwrap().capture(&snapshot());
                if self.closing.load(Ordering::SeqCst) {
                    return Err("Raesir is shutting down".into());
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        let lease = lock.as_mut().unwrap();
        lease.capture(&snapshot());
        if !lease.alive(&snapshot()) {
            return Err("Steam exited during startup. Check the client/runtime, stop the managed session and retry.".into());
        }
        // Protect the client before sending a mutation: shutdown may race the
        // reply, and an accepted Steam operation must survive that race.
        lease.keep_running |= keep_running;
        let result = probe(&lease.client.root);
        lease.capture(&snapshot());
        result
    }
    fn probe(&self, root: &str) -> Result<OwnedLibrary, String> {
        let output = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let mut child = Command::new(env::current_exe().map_err(|e| e.to_string())?)
            .args(["--steam-library-probe", root])
            .arg(output.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start the isolated Steam IPC helper")?;
        let until = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if self.closing.load(Ordering::SeqCst) || Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Steam lookup cancelled or timed out. Finish signing in to Steam, then retry. The previous library was kept.".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Cannot monitor the Steam IPC helper".into());
                }
            }
        };
        if !status.success() {
            return Err("The Steam IPC helper failed. This client build/runtime may be incompatible; the previous library was kept.".into());
        }
        let json = crate::steam::read_text(output.path())?;
        serde_json::from_str::<Result<OwnedLibrary, String>>(&json)
            .map_err(|_| "Steam IPC returned no valid result".to_string())?
    }
    pub fn stop(&self) -> Result<(), String> {
        // Dropping the lease always runs cleanup, including failed login/import.
        let mut lease = self.lease.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(client) = lease.as_mut() {
            client.stop()?;
        }
        lease.take();
        Ok(())
    }

    pub fn control(
        &self,
        root: &str,
        appid: u32,
        operation: &str,
    ) -> Result<steam_control::Status, steam_control::Error> {
        use steam_control::Error;
        if env::var_os("RAESIR_DISABLE_STEAM_IPC").is_some() {
            return Err(Error::Unavailable);
        }
        let client = clients()
            .into_iter()
            .find(|c| c.root == root)
            .ok_or(Error::Unavailable)?;
        self.read_client(
            client,
            matches!(operation, "install" | "uninstall" | "pause" | "resume"),
            |root| Ok(self.control_helper(root, appid, operation)),
        )
        .map_err(|_| Error::Unavailable)?
    }

    fn control_helper(
        &self,
        root: &str,
        appid: u32,
        operation: &str,
    ) -> Result<steam_control::Status, steam_control::Error> {
        use steam_control::Error;
        let output = tempfile::NamedTempFile::new().map_err(|_| Error::Unavailable)?;
        let mut child = Command::new(env::current_exe().map_err(|_| Error::Unavailable)?)
            .args(["--steam-control", root, operation, &appid.to_string()])
            .arg(output.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return Err(Error::Unavailable),
                Ok(None) if !self.closing.load(Ordering::SeqCst) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(30))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Unavailable);
                }
            }
        }
        let bytes = std::fs::read(output.path()).map_err(|_| Error::Unavailable)?;
        serde_json::from_slice::<Result<steam_control::Status, Error>>(&bytes)
            .map_err(|_| Error::Unavailable)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn existing_client_is_borrowed_and_survives_success_failure_and_cleanup() {
        struct Fixture(Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("steam");
        let mut external = Fixture(crate::test_process::spawn(&executable));
        let process = Pid::from_u32(external.0.id());
        assert!(is_steam(snapshot().process(process).unwrap().name()));
        let manager = Manager::default();
        let client = Client {
            root: dir.path().to_string_lossy().into(),
            label: "external fixture".into(),
            // Any attempt to start or shut down this client fails the test.
            program: dir.path().join("must-not-be-launched"),
            prefix: vec![],
        };
        for fail in [false, true] {
            let mut probed = false;
            let result = manager.read_client(client.clone(), false, |root| {
                probed = true;
                assert_eq!(root, client.root);
                if fail {
                    Err("fixture lookup failed".into())
                } else {
                    Ok(OwnedLibrary {
                        steamid: "fixture account".into(),
                        games: vec![],
                        source: "steam_client".into(),
                    })
                }
            });
            assert!(probed);
            assert_eq!(result.is_err(), fail);
            assert!(manager.lease.lock().unwrap().is_none());
            assert!(manager.status().external_running);
            manager.stop().unwrap();
            assert!(external.0.try_wait().unwrap().is_none());
        }
        drop(manager);
        assert!(external.0.try_wait().unwrap().is_none());
    }
    #[test]
    fn pid_reuse_is_not_a_matching_process_identity() {
        assert_ne!(
            Identity {
                pid: Pid::from_u32(42),
                started: 1
            },
            Identity {
                pid: Pid::from_u32(42),
                started: 2
            }
        );
        assert!(is_steam(std::ffi::OsStr::new("steam.exe")));
        assert!(!is_steam(std::ffi::OsStr::new("steamwebhelper")));
    }
    #[cfg(unix)]
    #[test]
    fn cleanup_only_terminates_the_owned_child() {
        let mut unrelated = Command::new("sleep").arg("20").spawn().unwrap();
        let child = Command::new("sleep").arg("20").spawn().unwrap();
        let id = Pid::from_u32(child.id());
        let client = Client {
            root: "/nonexistent-test-steam".into(),
            label: "fixture".into(),
            program: "/bin/false".into(),
            prefix: vec![],
        };
        let mut lease = Lease {
            client,
            child,
            owned: BTreeSet::new(),
            before: BTreeSet::new(),
            keep_running: false,
        };
        lease.capture(&snapshot());
        assert!(lease.owned.iter().any(|i| i.pid == id));
        lease.keep_running = true;
        lease.stop().unwrap();
        assert!(lease.child.try_wait().unwrap().is_none());
        lease.keep_running = false;
        lease.stop().unwrap();
        assert!(lease.child.try_wait().unwrap().is_some());
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }
}

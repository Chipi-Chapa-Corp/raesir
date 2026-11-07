//! Verify the owner before sending any request to a client interface.
use super::Client;
use std::{fs, path::Path};

pub(super) fn verified(port: u16, client: Client, root: &Path, prefix: Option<&Path>) -> bool {
    #[cfg(target_os = "linux")]
    {
        linux(port, client, root, prefix)
    }
    #[cfg(windows)]
    {
        let _ = prefix;
        windows(port, client, root)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (port, client, root, prefix);
        false
    }
}

#[cfg(target_os = "linux")]
fn linux(port: u16, client: Client, root: &Path, prefix: Option<&Path>) -> bool {
    use std::{collections::HashSet, os::unix::fs::MetadataExt};
    let Some(prefix) = prefix else { return false };
    if Path::new("/.flatpak-info").is_file() {
        use std::{
            io::Read,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let Ok(mut child) = Command::new("flatpak-spawn")
            .args(["--host", "python3", "-c", include_str!("peer.py")])
            .arg(port.to_string())
            .arg(client.store())
            .arg(root)
            .arg(prefix)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut result = String::new();
                    let _ = child
                        .stdout
                        .take()
                        .unwrap()
                        .take(16)
                        .read_to_string(&mut result);
                    return status.success() && result.trim() == "verified";
                }
                Err(_) => return false,
                _ if started.elapsed() > Duration::from_secs(3) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
    let prefix = prefix.canonicalize().unwrap_or_else(|_| prefix.into());
    let mut inodes = HashSet::new();
    let mut has_client = false;
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = entry.path();
        if !path
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
        {
            continue;
        }
        let Ok(comm) = fs::read_to_string(path.join("comm")) else {
            continue;
        };
        let comm = comm.trim();
        let is_client = comm.eq_ignore_ascii_case(client.executable());
        if !is_client && !comm.to_ascii_lowercase().starts_with("wineserver") {
            continue;
        }
        let Ok(environment) = fs::read(path.join("environ")) else {
            continue;
        };
        let Some(value) = environment
            .split(|b| *b == 0)
            .find_map(|v| v.strip_prefix(b"WINEPREFIX="))
        else {
            continue;
        };
        let Ok(value) = std::str::from_utf8(value) else {
            continue;
        };
        if Path::new(value).canonicalize().ok().as_ref() != Some(&prefix) {
            continue;
        }
        if is_client {
            // A Wine server is accepted only with the selected client running
            // in that same prefix. Other browser sessions are not candidates.
            let Ok(args) = fs::read(path.join("cmdline")) else {
                continue;
            };
            let Some(first) = args
                .split(|b| *b == 0)
                .next()
                .and_then(|v| std::str::from_utf8(v).ok())
            else {
                continue;
            };
            let mapped = if first.starts_with('/') {
                Some(first.into())
            } else {
                let raw = first.replace('\\', "/");
                if raw.as_bytes().get(1) == Some(&b':') {
                    prefix
                        .join("dosdevices")
                        .join(raw[..2].to_ascii_lowercase())
                        .canonicalize()
                        .ok()
                        .map(|drive| drive.join(raw[2..].trim_start_matches('/')))
                } else {
                    None
                }
            };
            if !mapped
                .as_ref()
                .and_then(|p: &std::path::PathBuf| p.parent())
                .is_some_and(|p| crate::epic::key(p) == crate::epic::key(root))
            {
                continue;
            }
            has_client = true;
        }
        for fd in fs::read_dir(path.join("fd"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if let Ok(link) = fs::read_link(fd.path()) {
                let link = link.to_string_lossy();
                if let Some(inode) = link
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                {
                    inodes.insert(inode.to_owned());
                }
            }
        }
    }
    has_client
        && ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|table| {
            fs::read_to_string(table).ok().is_some_and(|text| {
                text.lines().skip(1).any(|line| {
                    let parts: Vec<_> = line.split_whitespace().collect();
                    parts.len() > 9
                        && parts[3] == "0A"
                        && inodes.contains(parts[9])
                        && parts[1]
                            .rsplit(':')
                            .next()
                            .and_then(|p| u16::from_str_radix(p, 16).ok())
                            == Some(port)
                })
            })
        })
}

#[cfg(windows)]
fn windows(port: u16, client: Client, root: &Path) -> bool {
    use std::ffi::c_void;
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            ordered: i32,
            family: u32,
            class: u32,
            reserved: u32,
        ) -> u32;
    }
    let mut size = 0u32;
    // TCP_TABLE_OWNER_PID_LISTENER is the documented IPv4 owner table.
    if unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, 3, 0) } != 122
        || size > 4 * 1024 * 1024
    {
        return false;
    }
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    if unsafe { GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &mut size, 0, 2, 3, 0) } != 0 {
        return false;
    }
    let Some(&count) = buffer.first() else {
        return false;
    };
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    buffer[1..].chunks_exact(6).take(count as usize).any(|row| {
        if u16::from_be((row[2] & 0xffff) as u16) != port {
            return false;
        }
        let Some(process) = system.process(sysinfo::Pid::from_u32(row[5])) else {
            return false;
        };
        let Some(exe) = process.exe() else {
            return false;
        };
        let Some(name) = exe.file_name().map(|s| s.to_string_lossy()) else {
            return false;
        };
        (name.eq_ignore_ascii_case(client.executable())
            || name.eq_ignore_ascii_case(client.helper()))
            && exe
                .parent()
                .is_some_and(|parent| crate::epic::key(parent) == crate::epic::key(root))
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        net::TcpListener,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn native_peer_fixture() {
        let Some(dir) = std::env::var_os("RAESIR_NATIVE_PEER_FIXTURE") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        std::fs::write(
            dir.join("port"),
            listener.local_addr().unwrap().port().to_string(),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        while !dir.join("stop").exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn windows_listener_owner_must_be_the_selected_client_executable_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("EADesktop.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "client_downloads::peer::tests::native_peer_fixture",
            ])
            .env("RAESIR_NATIVE_PEER_FIXTURE", dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let port = loop {
            if let Ok(text) = std::fs::read_to_string(dir.path().join("port"))
                && let Ok(port) = text.parse::<u16>()
            {
                break port;
            }
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Fixture did not open its listener");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let matches = verified(port, Client::Ea, dir.path(), None);
        let wrong_client = verified(port, Client::Ubisoft, dir.path(), None);
        let wrong_root = verified(port, Client::Ea, &dir.path().join("other"), None);
        std::fs::write(dir.path().join("stop"), b"stop").unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
        assert!(matches);
        assert!(!wrong_client);
        assert!(!wrong_root);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod diagnostic {
    use super::*;

    #[test]
    #[ignore = "requires an agent-owned Ubisoft client on isolated-gui"]
    fn private_wine_listener_owner() {
        assert!(std::env::var_os("ISOLATED_GUI_SESSION").is_some());
        let root = std::path::PathBuf::from(
            std::env::var_os("RAESIR_NATIVE_CLIENT_ROOT").expect("private client root"),
        );
        let prefix = std::path::PathBuf::from(
            std::env::var_os("RAESIR_NATIVE_CLIENT_PREFIX").expect("private prefix"),
        );
        let private = crate::platform::home()
            .unwrap()
            .join(".cache/agent-desktops");
        assert!(root.starts_with(&private) && prefix.starts_with(&private));
        assert!(verified(9222, Client::Ubisoft, &root, Some(&prefix)));
        assert!(!verified(9222, Client::Ea, &root, Some(&prefix)));
        assert!(!verified(
            9222,
            Client::Ubisoft,
            &root.join("other"),
            Some(&prefix)
        ));
        assert!(!verified(
            9222,
            Client::Ubisoft,
            &root,
            Some(&prefix.join("other"))
        ));
    }
}

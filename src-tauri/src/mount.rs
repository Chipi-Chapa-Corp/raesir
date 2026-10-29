//! User-triggered mounts go through the OS service; no sudo or password capture.
use crate::{
    model::{Os, Volume},
    platform,
};

pub fn mount_options(filesystem: &str) -> Option<&'static str> {
    match filesystem.to_ascii_lowercase().as_str() {
        "ntfs" => Some("ro"),
        "ext4" => Some("ro,noload"),
        "btrfs" => Some("ro,nologreplay"),
        _ => None,
    }
}

pub fn validate_volume<'a>(volumes: &'a [Volume], id: &str) -> Result<&'a Volume, String> {
    let volume = volumes
        .iter()
        .find(|v| v.id == id)
        .ok_or("The partition changed or disappeared. Rescan first.")?;
    if !volume.mountpoints.is_empty() {
        return Err("This partition is already mounted. Rescan to read it.".into());
    }
    if mount_options(&volume.filesystem).is_none() {
        return Err(
            "Unlock encrypted storage or install its filesystem support using your OS tools first."
                .into(),
        );
    }
    if !volume.device.starts_with("/dev/") || volume.device.contains("..") {
        return Err("This is not a mountable Linux block device.".into());
    }
    Ok(volume)
}

pub fn mount_read_only(id: &str) -> Result<String, String> {
    if Os::host() != Os::Linux {
        return Err("Mounting through the system authorization dialog currently supports Linux. On Windows, expose the volume with its filesystem driver first.".into());
    }
    let (volumes, _) = platform::volumes();
    let volume = validate_volume(&volumes, id)?;
    #[cfg(target_os = "linux")]
    {
        system_mount(volume)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = volume;
        Err("Linux mount service is unavailable".into())
    }
}

#[cfg(target_os = "linux")]
fn system_mount(volume: &Volume) -> Result<String, String> {
    use zbus::blocking::{Connection, connection::Builder};
    let connection = Connection::system().or_else(|error| {
        // Toolbx exposes the host system socket here. This is used only after
        // the explicit mount click; authorization is still enforced by Polkit.
        if std::env::var_os("RAESIR_DISABLE_HOST_BRIDGE").is_some()
            || std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS").is_some()
            || !std::path::Path::new("/run/host/run/dbus/system_bus_socket").exists() { return Err(error); }
        Builder::address("unix:path=/run/host/run/dbus/system_bus_socket")?.build()
    }).map_err(|_| "Cannot reach UDisks on the system bus. Run Raesir in the desktop host session or expose its system bus to this container.")?;
    mount_with_connection(volume, &connection)
}

#[cfg(target_os = "linux")]
fn mount_with_connection(
    volume: &Volume,
    connection: &zbus::blocking::Connection,
) -> Result<String, String> {
    use std::collections::HashMap;
    use zbus::{
        blocking::Proxy,
        zvariant::{OwnedValue, Value},
    };
    let basename = volume
        .device
        .strip_prefix("/dev/")
        .ok_or("Invalid block device")?;
    let escaped: String = basename
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("_{b:02x}")
            }
        })
        .collect();
    let object = format!("/org/freedesktop/UDisks2/block_devices/{escaped}");
    let block = Proxy::new(
        connection,
        "org.freedesktop.UDisks2",
        object.as_str(),
        "org.freedesktop.UDisks2.Block",
    )
    .map_err(|e| format!("UDisks is unavailable: {e}"))?;
    let actual: Vec<u8> = block
        .get_property("Device")
        .map_err(|e| format!("Cannot inspect partition in UDisks: {e}"))?;
    if actual.strip_suffix(&[0]).unwrap_or(&actual) != volume.device.as_bytes() {
        return Err("The selected device changed; rescan first.".into());
    }
    let filesystem: String = block.get_property("IdType").map_err(|e| e.to_string())?;
    if !filesystem.eq_ignore_ascii_case(&volume.filesystem) {
        return Err("The filesystem changed; rescan first.".into());
    }
    // UDisks ignores caller mount options for fstab devices. Refuse those instead
    // of promising a read-only mount that the system might mount read-write.
    let configuration: Vec<(String, HashMap<String, OwnedValue>)> = block
        .get_property("Configuration")
        .map_err(|e| format!("Cannot inspect mount configuration: {e}"))?;
    if configuration.iter().any(|(kind, _)| kind == "fstab") {
        return Err("This partition has an /etc/fstab entry, so UDisks may ignore read-only options. Configure/mount it read-only using the OS, then rescan.".into());
    }
    let service = Proxy::new(
        connection,
        "org.freedesktop.UDisks2",
        object.as_str(),
        "org.freedesktop.UDisks2.Filesystem",
    )
    .map_err(|e| e.to_string())?;
    let mut options = HashMap::new();
    options.insert(
        "options",
        Value::from(mount_options(&filesystem).ok_or("Unsupported filesystem")?),
    );
    // Omitting auth.no_user_interaction allows the desktop's Polkit agent to
    // ask for approval/password. Raesir never receives that password.
    service.call("Mount", &(options,)).map_err(|e| format!("Mount was cancelled or failed: {e}. Check UDisks, the filesystem driver and your desktop authorization agent. No writable fallback was attempted."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_known_unmounted_devices_can_be_requested() {
        let mut volume = Volume {
            id: "disk".into(),
            device: "/dev/test1".into(),
            filesystem: "ntfs".into(),
            ..Volume::default()
        };
        assert!(validate_volume(&[volume.clone()], "other").is_err());
        assert!(validate_volume(&[volume.clone()], "disk").is_ok());
        volume.mountpoints.push("/mnt/test".into());
        assert!(validate_volume(&[volume.clone()], "disk").is_err());
        volume.mountpoints.clear();
        volume.filesystem = "crypto_LUKS".into();
        assert!(validate_volume(&[volume], "disk").is_err());
    }
    #[test]
    fn readonly_options_never_repair_or_replay_journals() {
        assert_eq!(mount_options("ntfs"), Some("ro"));
        assert_eq!(mount_options("ext4"), Some("ro,noload"));
        assert_eq!(mount_options("btrfs"), Some("ro,nologreplay"));
        assert_eq!(mount_options("unknown"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_dbus_fixture_enforces_readonly_and_refuses_fstab() {
        use std::{
            collections::HashMap,
            io::{BufRead, BufReader},
            process::{Command, Stdio},
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
            },
        };
        use zbus::{blocking::connection::Builder, zvariant::OwnedValue};
        struct Daemon(std::process::Child);
        impl Drop for Daemon {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut daemon = Daemon(
            Command::new("/usr/bin/dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let fstab = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        struct Block {
            fstab: Arc<AtomicBool>,
        }
        #[zbus::interface(name = "org.freedesktop.UDisks2.Block")]
        impl Block {
            #[zbus(property)]
            fn device(&self) -> Vec<u8> {
                b"/dev/raesir_fixture\0".to_vec()
            }
            #[zbus(property)]
            fn id_type(&self) -> String {
                "ntfs".into()
            }
            #[zbus(property)]
            fn configuration(&self) -> Vec<(String, HashMap<String, OwnedValue>)> {
                if self.fstab.load(Ordering::SeqCst) {
                    vec![("fstab".into(), HashMap::new())]
                } else {
                    vec![]
                }
            }
        }
        struct Filesystem {
            calls: Arc<AtomicUsize>,
        }
        #[zbus::interface(name = "org.freedesktop.UDisks2.Filesystem")]
        impl Filesystem {
            fn mount(&self, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<String> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let value = options
                    .get("options")
                    .and_then(|v| <&str>::try_from(v).ok());
                if value != Some("ro") || options.len() != 1 {
                    return Err(zbus::fdo::Error::Failed(
                        "unexpected mount arguments".into(),
                    ));
                }
                Ok("/fixture/mounted-readonly".into())
            }
        }
        let path = "/org/freedesktop/UDisks2/block_devices/raesir_5ffixture";
        let _server = Builder::address(address.trim())
            .unwrap()
            .name("org.freedesktop.UDisks2")
            .unwrap()
            .serve_at(
                path,
                Block {
                    fstab: fstab.clone(),
                },
            )
            .unwrap()
            .serve_at(
                path,
                Filesystem {
                    calls: calls.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        let connection = Builder::address(address.trim()).unwrap().build().unwrap();
        let volume = Volume {
            device: "/dev/raesir_fixture".into(),
            filesystem: "ntfs".into(),
            ..Volume::default()
        };
        assert_eq!(
            mount_with_connection(&volume, &connection).unwrap(),
            "/fixture/mounted-readonly"
        );
        fstab.store(true, Ordering::SeqCst);
        assert!(
            mount_with_connection(&volume, &connection)
                .unwrap_err()
                .contains("fstab")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

//! User-triggered mounts go through the OS service; no sudo or password capture.
use crate::{model::Volume, platform};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub struct Manager {
    mounts: Mutex<Vec<Mounted>>,
    closing: AtomicBool,
}

#[derive(Debug)]
struct Mounted {
    volume: Volume,
    point: String,
    uuid: String,
    fingerprint: Option<String>,
    #[cfg(target_os = "linux")]
    connection: zbus::blocking::Connection,
}

impl Manager {
    pub fn mount(&self, id: &str) -> Result<String, String> {
        let mut mounts = self.mounts.lock().unwrap_or_else(|e| e.into_inner());
        if self.closing.load(Ordering::SeqCst) {
            return Err("Raesir is closing".into());
        }
        let (volumes, _) = platform::volumes();
        let volume = validate_volume(&volumes, id)?;
        #[cfg(target_os = "linux")]
        {
            let mounted = mount_with_connection(volume, &system_connection()?)?;
            let point = mounted.point.clone();
            mounts.push(mounted);
            Ok(point)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (volume, &mut mounts);
            Err(
                "Mounting currently supports Linux; use the OS filesystem driver on Windows."
                    .into(),
            )
        }
    }
    pub fn begin_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.closing.store(false, Ordering::SeqCst);
    }
    pub fn shutdown(&self) -> Result<(), String> {
        self.begin_closing();
        let mut mounts = self.mounts.lock().unwrap_or_else(|e| e.into_inner());
        let mut errors = Vec::new();
        for i in (0..mounts.len()).rev() {
            match mounts[i].unmount() {
                Ok(()) => {
                    mounts.remove(i);
                }
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Could not unmount Raesir's partitions: {}. Close any games/files using them and retry closing Raesir.",
                errors.join("; ")
            ))
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(target_os = "linux")]
fn fingerprint(point: &str) -> Option<String> {
    let info = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    for line in info.lines() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        let actual = fields
            .get(4)?
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        if actual == point || actual == format!("/run/host{point}") {
            return Some(format!("{}:{}:{}", fields[0], fields[2], fields[3]));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn object(volume: &Volume) -> Result<String, String> {
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
    Ok(format!("/org/freedesktop/UDisks2/block_devices/{escaped}"))
}

impl Mounted {
    #[cfg(target_os = "linux")]
    fn unmount(&self) -> Result<(), String> {
        use std::collections::HashMap;
        use zbus::{blocking::Proxy, zvariant::Value};
        let object = object(&self.volume)?;
        let service = Proxy::new(
            &self.connection,
            "org.freedesktop.UDisks2",
            object.as_str(),
            "org.freedesktop.UDisks2.Filesystem",
        )
        .map_err(|e| e.to_string())?;
        let points: Vec<Vec<u8>> = service
            .get_property("MountPoints")
            .map_err(|e| e.to_string())?;
        // Someone may have unmounted it manually. Never unmount a replacement.
        if !points
            .iter()
            .any(|p| p.strip_suffix(&[0]).unwrap_or(p) == self.point.as_bytes())
        {
            return Ok(());
        }
        if let (Some(before), Some(now)) = (&self.fingerprint, fingerprint(&self.point))
            && before != &now
        {
            return Ok(());
        }
        let block = Proxy::new(
            &self.connection,
            "org.freedesktop.UDisks2",
            object.as_str(),
            "org.freedesktop.UDisks2.Block",
        )
        .map_err(|e| e.to_string())?;
        let uuid: String = block.get_property("IdUUID").map_err(|e| e.to_string())?;
        let device: Vec<u8> = block.get_property("Device").map_err(|e| e.to_string())?;
        if uuid != self.uuid
            || device.strip_suffix(&[0]).unwrap_or(&device) != self.volume.device.as_bytes()
        {
            return Ok(());
        }
        // No force/lazy unmount. Busy filesystems keep the app open for retry.
        let result: Result<(), _> = service.call("Unmount", &(HashMap::<&str, Value<'_>>::new(),));
        result.map_err(|e| format!("{}: {e}", self.point))
    }
    #[cfg(not(target_os = "linux"))]
    fn unmount(&self) -> Result<(), String> {
        let _ = (&self.volume, &self.point, &self.uuid, &self.fingerprint);
        Ok(())
    }
}

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

#[cfg(target_os = "linux")]
fn system_connection() -> Result<zbus::blocking::Connection, String> {
    use zbus::blocking::{Connection, connection::Builder};
    Connection::system().or_else(|error| {
        // Toolbx exposes the host system socket here. This is used only after
        // the explicit mount click; authorization is still enforced by Polkit.
        if std::env::var_os("RAESIR_DISABLE_HOST_BRIDGE").is_some()
            || std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS").is_some()
            || !std::path::Path::new("/run/host/run/dbus/system_bus_socket").exists() { return Err(error); }
        Builder::address("unix:path=/run/host/run/dbus/system_bus_socket")?.build()
    }).map_err(|_| "Cannot reach UDisks on the system bus. Run Raesir in the desktop host session or expose its system bus to this container.".into())
}

#[cfg(target_os = "linux")]
fn mount_with_connection(
    volume: &Volume,
    connection: &zbus::blocking::Connection,
) -> Result<Mounted, String> {
    use std::collections::HashMap;
    use zbus::{
        blocking::Proxy,
        zvariant::{OwnedValue, Value},
    };
    let object = object(volume)?;
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
    let uuid: String = block.get_property("IdUUID").map_err(|e| e.to_string())?;
    if !uuid.is_empty() && !volume.id.is_empty() && volume.id != volume.device && volume.id != uuid
    {
        return Err("The selected partition identity changed; rescan first.".into());
    }
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
    let points: Vec<Vec<u8>> = service
        .get_property("MountPoints")
        .map_err(|e| e.to_string())?;
    if !points.is_empty() {
        return Err("This partition was mounted elsewhere. Rescan to read it.".into());
    }
    let mut options = HashMap::new();
    options.insert(
        "options",
        Value::from(mount_options(&filesystem).ok_or("Unsupported filesystem")?),
    );
    // Omitting auth.no_user_interaction allows the desktop's Polkit agent to
    // ask for approval/password. Raesir never receives that password.
    let point: String = service.call("Mount", &(options,)).map_err(|e| format!("Mount was cancelled or failed: {e}. Check UDisks, the filesystem driver and your desktop authorization agent. No writable fallback was attempted."))?;
    Ok(Mounted {
        volume: volume.clone(),
        fingerprint: fingerprint(&point),
        point,
        uuid,
        connection: connection.clone(),
    })
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
        let mounted = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let unmounts = Arc::new(AtomicUsize::new(0));
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
            #[zbus(property, name = "IdUUID")]
            fn id_uuid(&self) -> String {
                "fixture-uuid".into()
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
            mounted: Arc<AtomicBool>,
            busy: Arc<AtomicBool>,
            unmounts: Arc<AtomicUsize>,
        }
        #[zbus::interface(name = "org.freedesktop.UDisks2.Filesystem")]
        impl Filesystem {
            #[zbus(property)]
            fn mount_points(&self) -> Vec<Vec<u8>> {
                if self.mounted.load(Ordering::SeqCst) {
                    vec![b"/fixture/mounted-readonly\0".to_vec()]
                } else {
                    vec![]
                }
            }
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
                self.mounted.store(true, Ordering::SeqCst);
                Ok("/fixture/mounted-readonly".into())
            }
            fn unmount(&self, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<()> {
                assert!(options.is_empty()); // Never force/lazy unmount.
                self.unmounts.fetch_add(1, Ordering::SeqCst);
                if self.busy.load(Ordering::SeqCst) {
                    return Err(zbus::fdo::Error::Failed("volume busy".into()));
                }
                self.mounted.store(false, Ordering::SeqCst);
                Ok(())
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
                    mounted: mounted.clone(),
                    busy: busy.clone(),
                    unmounts: unmounts.clone(),
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
        let lease = mount_with_connection(&volume, &connection).unwrap();
        assert_eq!(lease.point, "/fixture/mounted-readonly");
        // Already-mounted partitions are never adopted by Raesir.
        assert!(
            mount_with_connection(&volume, &connection)
                .unwrap_err()
                .contains("mounted elsewhere")
        );
        let manager = Manager::default();
        manager.mounts.lock().unwrap().push(lease);
        busy.store(true, Ordering::SeqCst);
        assert!(manager.shutdown().unwrap_err().contains("busy"));
        assert_eq!(manager.mounts.lock().unwrap().len(), 1);
        busy.store(false, Ordering::SeqCst);
        manager.shutdown().unwrap();
        assert_eq!(unmounts.load(Ordering::SeqCst), 2);
        assert!(!mounted.load(Ordering::SeqCst));
        manager.shutdown().unwrap();
        assert_eq!(unmounts.load(Ordering::SeqCst), 2);
        // A pre-existing mount has no lease and survives app cleanup.
        mounted.store(true, Ordering::SeqCst);
        Manager::default().shutdown().unwrap();
        assert!(mounted.load(Ordering::SeqCst));
        mounted.store(false, Ordering::SeqCst);
        fstab.store(true, Ordering::SeqCst);
        assert!(
            mount_with_connection(&volume, &connection)
                .unwrap_err()
                .contains("fstab")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

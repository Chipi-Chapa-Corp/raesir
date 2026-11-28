use crate::model::{Os, Source, Volume};
use serde_json::Value;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn home() -> Option<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

pub fn steam_in_home(home: &Path) -> Vec<PathBuf> {
    [
        ".local/share/Steam",
        ".local/share/steam",
        ".steam/steam",
        ".steam/root",
        ".steam/debian-installation",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        ".var/app/com.valvesoftware.Steam/data/Steam",
        ".var/app/com.valvesoftware.Steam/data/steam",
        "snap/steam/common/.local/share/Steam",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

pub fn local_sources() -> Vec<Source> {
    let mut paths = Vec::new();
    if let Some(home) = home() {
        paths.extend(steam_in_home(&home));
    }
    if let Some(data) = env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data).join("Steam"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(out) = Command::new("reg")
            .creation_flags(0x08000000)
            .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
            .output()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some((_, value)) = line.split_once("REG_SZ") {
                    paths.push(PathBuf::from(value.trim()));
                }
            }
        }
        for key in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(p) = env::var_os(key) {
                paths.push(PathBuf::from(p).join("Steam"));
            }
        }
    }
    paths
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| Source {
            path: p.to_string_lossy().into(),
            os: Os::host(),
        })
        .collect()
}

fn children(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .take(256)
        .collect()
}

pub fn os_at(root: &Path) -> Os {
    if root.join("Windows/System32").is_dir() {
        Os::Windows
    } else if root.join("etc/os-release").is_file() || root.join("usr/lib/os-release").is_file() {
        Os::Linux
    } else {
        Os::Unknown
    }
}

pub fn scan_mount(path: &Path) -> bool {
    !path.starts_with("/run/host")
        || [
            "/run/host/run/media",
            "/run/host/media",
            "/run/host/mnt",
            "/run/host/var/mnt",
        ]
        .iter()
        .any(|p| path.starts_with(p))
}

/// Toolbx can expose the same host mount at both its original path and under
/// /run/host. Compare inode identity before preferring the original path.
pub(crate) fn host_mount_alias(path: &str) -> Option<&str> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let original = path.strip_prefix("/run/host")?;
        if !original.starts_with('/') || !scan_mount(Path::new(path)) {
            return None;
        }
        let alias = fs::metadata(path).ok()?;
        let native = fs::metadata(original).ok()?;
        (alias.dev() == native.dev() && alias.ino() == native.ino()).then_some(original)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

/// Bounded, known locations only. Never recursively crawl game payloads or follow
/// a foreign /home symlink into the running OS's /home.
pub fn sources_at(root: &Path, hint: Os) -> Vec<Source> {
    let mut found = Vec::new();
    let os = match os_at(root) {
        Os::Unknown => hint,
        os => os,
    };
    let mut candidates: Vec<(PathBuf, Os)> = vec![(root.into(), os)];
    for p in [
        "Program Files (x86)/Steam",
        "Program Files/Steam",
        "Steam",
        "SteamLibrary",
        "Games/Steam",
        "Games/SteamLibrary",
    ] {
        candidates.push((root.join(p), os));
    }
    for directory in ["home", "var/home", "@home", "Users"] {
        let parent = root.join(directory);
        if parent.is_symlink() {
            continue;
        }
        for user in children(&parent) {
            if user.is_symlink() {
                continue;
            }
            candidates.extend(steam_in_home(&user).into_iter().map(|p| (p, Os::Linux)));
        }
    }
    // A mounted home partition exposes users directly.
    if !root.join("etc").exists() && !root.join("Windows").exists() {
        for user in children(root) {
            if !user.is_symlink() {
                candidates.extend(steam_in_home(&user).into_iter().map(|p| (p, Os::Linux)));
            }
        }
    }
    // Common Btrfs root-subvolume layout.
    if root.join("@").is_dir() && !root.join("@").is_symlink() {
        let sub = root.join("@");
        for p in ["home", "var/home"] {
            let parent = sub.join(p);
            if !parent.is_symlink() {
                for user in children(&parent) {
                    if !user.is_symlink() {
                        candidates.extend(steam_in_home(&user).into_iter().map(|p| (p, Os::Linux)));
                    }
                }
            }
        }
    }
    for (path, mut os) in candidates {
        if path.join("steam.exe").is_file() {
            os = Os::Windows;
        }
        if path.join("steamapps").is_dir()
            || path
                .file_name()
                .is_some_and(|p| p.eq_ignore_ascii_case("steamapps"))
        {
            found.push(Source {
                path: path.to_string_lossy().into(),
                os,
            });
        }
    }
    found
}

pub fn unreadable_note(host: Os, filesystem: &str, driver_present: bool) -> String {
    match (host, filesystem.to_ascii_lowercase().as_str()) {
        (Os::Windows, "btrfs") if !driver_present =>
            "WinBtrfs was not detected.".into(),
        (Os::Windows, "btrfs") =>
            "WinBtrfs is installed. Mount read-only in Windows.".into(),
        (Os::Windows, "ext4") =>
            "ext4 is not readable here. A filesystem reader/driver is required; Raesir scans readable mounted partitions. WSL --mount cannot attach the running Windows disk.".into(),
        (_, "crypto_luks" | "bitlocker") =>
            "Encrypted partition. Unlock and expose it using your OS tools, then rescan. Raesir does not unlock volumes.".into(),
        (Os::Linux, "ntfs") =>
            "Unmounted NTFS".into(),
        (Os::Windows, "linux") =>
            "Needs a Linux filesystem reader.".into(),
        _ => "Mount read-only in your OS.".into(),
    }
}

pub fn volumes() -> (Vec<Volume>, Vec<String>) {
    #[cfg(target_os = "linux")]
    {
        let flatpak = in_flatpak();
        if flatpak && env::var_os("RAESIR_DISABLE_HOST_BRIDGE").is_some() {
            return (
                Vec::new(),
                vec!["Flatpak host partition discovery is disabled.".into()],
            );
        }
        match lsblk_command(flatpak).output() {
            Ok(out) if out.status.success() => match serde_json::from_slice::<Value>(&out.stdout) {
                Ok(v) => return (parse_lsblk(&v), Vec::new()),
                Err(_) => return (Vec::new(), vec!["Could not parse lsblk output; automatic partition discovery is unavailable. Add readable paths manually.".into()]),
            },
            _ => return (Vec::new(), vec![if flatpak {
                "Host lsblk could not run through Flatpak; automatic partition discovery is unavailable. Check that util-linux is installed on the host or add readable paths manually.".into()
            } else {
                "lsblk is missing or failed; automatic partition discovery is unavailable. Install util-linux or add readable paths manually.".into()
            }]),
        }
    }
    #[cfg(windows)]
    {
        return windows_volumes();
    }
    #[allow(unreachable_code)]
    (
        Vec::new(),
        vec!["Automatic storage detection currently supports Linux and Windows only.".into()],
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").is_file()
}

#[cfg(target_os = "linux")]
fn lsblk_command(flatpak: bool) -> Command {
    // lsblk needs the host's device metadata and mount namespace to report
    // filesystem UUIDs and mounts for devices outside the sandbox.
    let mut command = if flatpak {
        let mut command = Command::new("flatpak-spawn");
        command.args(["--host", "lsblk"]);
        command
    } else {
        Command::new("lsblk")
    };
    command.args([
        "--json",
        "--bytes",
        "--output",
        "PATH,FSTYPE,LABEL,UUID,PARTTYPE,SIZE,MOUNTPOINTS",
    ]);
    command
}

pub fn parse_lsblk(data: &Value) -> Vec<Volume> {
    fn visit(value: &Value, output: &mut Vec<Volume>) {
        let filesystem = value["fstype"].as_str().unwrap_or("");
        let partition_type = value["parttype"].as_str().unwrap_or("");
        // Windows RE uses a designated GPT type or MBR type 0x27; Linux XBOOTLDR
        // (/boot) uses bc13c2ff-59e6-4262-a352-b275fd6f7172.
        let system_partition = partition_type
            .eq_ignore_ascii_case("de94bba4-06d1-4d40-a16a-bfd50179d6ac")
            || partition_type.eq_ignore_ascii_case("0x27")
            || partition_type.eq_ignore_ascii_case("bc13c2ff-59e6-4262-a352-b275fd6f7172");
        if ["ntfs", "btrfs", "ext4", "crypto_LUKS", "BitLocker"]
            .iter()
            .any(|s| s.eq_ignore_ascii_case(filesystem))
            && !system_partition
        {
            let raw_mountpoints: Vec<&str> = value["mountpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|p| p.starts_with('/'))
                .collect();
            let mounted = !raw_mountpoints.is_empty();
            let mountpoints = raw_mountpoints
                .into_iter()
                .filter(|p| Path::new(p).is_dir())
                .map(String::from)
                .collect();
            let uuid = value["uuid"].as_str().filter(|s| !s.is_empty());
            let device = value["path"].as_str().unwrap_or("unknown");
            let volume = Volume {
                id: uuid.unwrap_or(device).into(),
                device: device.into(),
                filesystem: filesystem.into(),
                label: value["label"]
                    .as_str()
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned),
                size_bytes: value["size"]
                    .as_u64()
                    .or_else(|| value["size"].as_str()?.parse().ok()),
                mountpoints,
                mounted,
                ..Volume::default()
            };
            // Btrfs reports the same filesystem UUID on every member, but
            // lsblk may attach mount points to only one of those members.
            if filesystem.eq_ignore_ascii_case("btrfs")
                && let Some(uuid) = uuid
                && let Some(pool) = output.iter_mut().find(|other| {
                    other.filesystem.eq_ignore_ascii_case("btrfs") && other.id == uuid
                })
            {
                if pool.device != device && !pool.additional_devices.contains(&volume.device) {
                    pool.additional_devices.push(volume.device);
                    pool.size_bytes = pool
                        .size_bytes
                        .zip(volume.size_bytes)
                        .and_then(|(total, size)| total.checked_add(size));
                }
                pool.label = pool.label.take().or(volume.label);
                pool.mounted |= volume.mounted;
                pool.mountpoints.extend(volume.mountpoints);
            } else {
                output.push(volume);
            }
        }
        for child in value["children"].as_array().into_iter().flatten() {
            visit(child, output);
        }
    }
    let mut result = Vec::new();
    for v in data["blockdevices"].as_array().into_iter().flatten() {
        visit(v, &mut result);
    }
    for volume in &mut result {
        let known: HashSet<_> = volume.mountpoints.iter().cloned().collect();
        let mut seen = HashSet::new();
        volume.mountpoints.retain(|path| {
            host_mount_alias(path).is_none_or(|original| !known.contains(original))
                && seen.insert(path.clone())
        });
        volume.os = volume
            .mountpoints
            .iter()
            .map(|p| os_at(Path::new(p)))
            .find(|os| *os != Os::Unknown)
            .unwrap_or(Os::Unknown);
        volume.note = if !volume.mountpoints.is_empty() {
            "Readable mount points will be checked for Steam libraries.".into()
        } else if volume.mounted {
            "Mounted on the host outside this sandbox.".into()
        } else {
            unreadable_note(Os::Linux, &volume.filesystem, false)
        };
    }
    result
}

#[cfg(windows)]
fn windows_volumes() -> (Vec<Volume>, Vec<String>) {
    use std::os::windows::process::CommandExt;
    // Fixed script: no user-controlled paths/commands are interpolated.
    let script = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding
$btrfs = Test-Path 'HKLM:\SYSTEM\CurrentControlSet\Services\btrfs'
$signatures = @{}
Get-Disk -ErrorAction SilentlyContinue | ForEach-Object { $signatures[[int]$_.Number] = $_.Signature }
$rows = @(Get-Partition | ForEach-Object {
    $p = $_; $v = $p | Get-Volume -ErrorAction SilentlyContinue
    $linux = "$($p.GptType)" -eq '{0fc63daf-8483-4772-8e79-3d69d8477de4}' -or $p.MbrType -eq 131
    $recovery = "$($p.GptType)" -eq '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}' -or $p.MbrType -eq 39
    if (($v -or $linux) -and -not $recovery) {
        $signature = $signatures[[int]$p.DiskNumber]
        [PSCustomObject]@{
            id = if ($p.Guid -and [guid]$p.Guid -ne [guid]::Empty) { "partuuid:$($p.Guid)" } elseif ($signature) { "mbr:${signature}:$($p.Offset)" } else { "windows-device:$($p.DiskNumber):$($p.PartitionNumber)" }
            disk = $p.DiskNumber; offset = $p.Offset
            partition = $p.PartitionNumber
            label = $v.FileSystemLabel
            filesystem = if ($v.FileSystem) { "$($v.FileSystem)" } elseif ($v.FileSystemType -and "$($v.FileSystemType)" -ne 'Unknown') { "$($v.FileSystemType)" } elseif ($linux) { 'linux' } else { 'unknown' }
            mounts = @($p.AccessPaths | Where-Object { $_ -match '^[A-Za-z]:\\' })
            btrfs = $btrfs
        }
    }
})
ConvertTo-Json -Depth 4 -Compress -InputObject $rows
"#;
    let out = Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    let data = match out {
        Ok(out) if out.status.success() => serde_json::from_slice::<Value>(&out.stdout).ok(),
        _ => None,
    };
    let Some(Value::Array(rows)) = data else {
        return (Vec::new(), vec!["Windows partition discovery failed. PowerShell with the Storage module is required; add readable paths manually.".into()]);
    };
    let mut result = Vec::new();
    for row in rows {
        let mut filesystem = row["filesystem"]
            .as_str()
            .unwrap_or("unknown")
            .to_lowercase();
        let disk = row["disk"].as_u64().unwrap_or(0);
        let partition = row["partition"].as_u64().unwrap_or(0);
        let offset = row["offset"].as_u64().unwrap_or(0);
        if filesystem == "linux" {
            if let Some(detected) = probe_linux_filesystem(disk, offset) {
                filesystem = detected.into();
            }
        }
        let mounts: Vec<String> = row["mounts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|p| Path::new(p).is_dir())
            .map(String::from)
            .collect();
        let os = mounts
            .iter()
            .map(|p| os_at(Path::new(p)))
            .find(|o| *o != Os::Unknown)
            .unwrap_or(Os::Unknown);
        let note = if mounts.is_empty() {
            unreadable_note(
                Os::Windows,
                &filesystem,
                row["btrfs"].as_bool().unwrap_or(false),
            )
        } else {
            "Readable mount points will be checked for Steam libraries.".into()
        };
        result.push(Volume {
            id: row["id"].as_str().unwrap_or("unknown").into(),
            device: format!("Disk {disk} · Partition {partition}"),
            label: row["label"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            filesystem,
            mountpoints: mounts,
            os,
            note,
            ..Volume::default()
        });
    }
    (result, Vec::new())
}

#[cfg(windows)]
fn probe_linux_filesystem(disk: u64, offset: u64) -> Option<&'static str> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(format!(r"\\.\PhysicalDrive{disk}")).ok()?;
    let mut magic = [0u8; 8];
    file.seek(SeekFrom::Start(offset.checked_add(0x10040)?))
        .ok()?;
    file.read_exact(&mut magic).ok()?;
    if &magic == b"_BHRfS_M" {
        return Some("btrfs");
    }
    file.seek(SeekFrom::Start(offset.checked_add(1080)?)).ok()?;
    file.read_exact(&mut magic[..2]).ok()?;
    if magic[..2] == [0x53, 0xef] {
        return Some("ext4");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn flatpak_discovery_runs_the_same_lsblk_query_on_the_host() {
        let native = lsblk_command(false);
        let flatpak = lsblk_command(true);
        let mut host_args = flatpak.get_args();
        assert_eq!(native.get_program(), "lsblk");
        assert_eq!(flatpak.get_program(), "flatpak-spawn");
        assert_eq!(host_args.next().unwrap(), "--host");
        assert_eq!(host_args.next().unwrap(), "lsblk");
        assert!(host_args.eq(native.get_args()));
    }

    #[test]
    fn unmounted_partition_is_not_a_confirmed_windows_install() {
        let data = serde_json::json!({"blockdevices":[{"path":"/dev/test", "children":[
            {"path":"/dev/test1","fstype":"ntfs","uuid":"abc","mountpoints":[null]},
            {"path":"/dev/test2","fstype":"swap","mountpoints":["[SWAP]"]}]}]});
        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].os, Os::Unknown);
        assert_eq!(volumes[0].note, "Unmounted NTFS");
    }

    #[test]
    fn linux_discovery_identifies_data_partitions_and_excludes_system_partitions() {
        let data = serde_json::json!({"blockdevices":[
            {"path":"/dev/test1","fstype":"ntfs","label":"Games","size":602825523200_u64},
            {"path":"/dev/test2","fstype":"ntfs","parttype":"DE94BBA4-06D1-4D40-A16A-BFD50179D6AC"},
            {"path":"/dev/test3","fstype":"ntfs","parttype":"0x27"},
            {"path":"/dev/test4","fstype":"ext4","parttype":"BC13C2FF-59E6-4262-A352-B275FD6F7172","mountpoints":["/boot"]},
            {"path":"/dev/test5","fstype":"ntfs","size":"1073741824"}
        ]});
        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 2);
        assert_eq!(volumes[0].device, "/dev/test1");
        assert_eq!(volumes[0].label.as_deref(), Some("Games"));
        assert_eq!(volumes[0].size_bytes, Some(602825523200));
        assert_eq!(volumes[1].device, "/dev/test5");
        assert_eq!(volumes[1].size_bytes, Some(1073741824));
    }

    #[test]
    fn host_mounted_partition_hidden_from_sandbox_stays_marked_mounted() {
        let data = serde_json::json!({"blockdevices":[
            {"path":"/dev/test1","fstype":"ext4","uuid":"boot-like","mountpoints":["/raesir-nonexistent-host-mount"]},
            {"path":"/dev/test2","fstype":"btrfs","uuid":"pool-hidden","size":1024,"mountpoints":[null]},
            {"path":"/dev/test3","fstype":"btrfs","uuid":"pool-hidden","size":2048,"mountpoints":["/raesir-nonexistent-pool-mount"]}
        ]});
        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 2);
        for volume in &volumes {
            assert!(volume.mountpoints.is_empty());
            assert!(volume.mounted);
            assert!(volume.is_mounted());
            assert!(
                crate::mount::validate_volume(&volumes, &volume.id)
                    .unwrap_err()
                    .contains("already mounted")
            );
        }
    }

    #[test]
    fn btrfs_members_share_mounts_and_count_each_device_once() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        let home = fixture.path().join("home");
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(root.join("etc/os-release"), "ID=linux\n").unwrap();
        fs::create_dir(&home).unwrap();
        let data = serde_json::json!({"blockdevices":[
            {"path":"/dev/test1", "fstype":"btrfs", "uuid":"pool", "size":1024},
            {"path":"/dev/test2", "fstype":"btrfs", "uuid":"pool", "size":"2048",
             "label":"Fedora", "mountpoints":[home]},
            {"children":[{"path":"/dev/test2", "fstype":"btrfs", "uuid":"pool", "size":2048,
                          "mountpoints":[home, root]}]},
            {"path":"/dev/test3", "fstype":"btrfs", "uuid":"other", "label":"Fedora", "size":1024},
            {"path":"/dev/test4", "fstype":"btrfs", "label":"Fedora", "size":1024}
        ]});

        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 3);
        let pool = &volumes[0];
        assert_eq!(pool.id, "pool");
        assert_eq!(pool.device, "/dev/test1");
        assert_eq!(pool.additional_devices, ["/dev/test2"]);
        assert_eq!(pool.label.as_deref(), Some("Fedora"));
        assert_eq!(pool.size_bytes, Some(3072));
        assert_eq!(
            pool.mountpoints,
            [home.to_str().unwrap(), root.to_str().unwrap()]
        );
        assert_eq!(pool.os, Os::Linux);
        assert!(
            crate::mount::validate_volume(&volumes, "pool")
                .unwrap_err()
                .contains("already mounted")
        );
        assert!(
            volumes[1..]
                .iter()
                .all(|volume| volume.additional_devices.is_empty())
        );
    }

    #[test]
    fn unmounted_btrfs_pool_has_one_read_only_mount_target() {
        let data = serde_json::json!({"blockdevices":[
            {"path":"/dev/test1", "fstype":"btrfs", "uuid":"pool", "size":1024},
            {"path":"/dev/test2", "fstype":"btrfs", "uuid":"pool"}
        ]});

        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 1);
        let pool = crate::mount::validate_volume(&volumes, "pool").unwrap();
        assert_eq!(pool.device, "/dev/test1");
        assert_eq!(pool.size_bytes, None);
    }

    #[test]
    fn missing_reader_has_actionable_message() {
        assert!(unreadable_note(Os::Windows, "btrfs", false).contains("WinBtrfs was not detected"));
        assert!(unreadable_note(Os::Windows, "btrfs", true).contains("installed"));
        assert!(unreadable_note(Os::Windows, "ext4", false).contains("reader/driver"));
    }
}

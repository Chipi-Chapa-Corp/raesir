use crate::model::{Os, Source, Volume};
use serde_json::Value;
use std::{
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
            "Btrfs cannot be read: WinBtrfs was not detected. Install/configure WinBtrfs in read-only mode, then rescan or add its mounted path.".into(),
        (Os::Windows, "btrfs") =>
            "WinBtrfs is installed, but this partition has no accessible mount. Expose the correct subvolume in read-only mode and rescan.".into(),
        (Os::Windows, "ext4") =>
            "ext4 is not readable here. A filesystem reader/driver is required; Raesir scans readable mounted partitions. WSL --mount cannot attach the running Windows disk.".into(),
        (_, "crypto_luks" | "bitlocker") =>
            "Encrypted partition. Unlock and expose it using your OS tools, then rescan. Raesir does not unlock volumes.".into(),
        (Os::Linux, "ntfs") =>
            "Unmounted NTFS partition; it may contain Windows games. Use Mount read-only to request access through your OS. Windows/Steam installation is not yet verified.".into(),
        (Os::Windows, "linux") =>
            "Linux partition detected; filesystem cannot be identified without raw-disk read access. Btrfs needs WinBtrfs; ext4 needs a reader/driver. Expose it read-only and add its path. WSL --mount cannot attach the running Windows disk.".into(),
        _ => "Partition is not mounted/readable. Expose it read-only using your OS tools, then rescan.".into(),
    }
}

pub fn volumes() -> (Vec<Volume>, Vec<String>) {
    #[cfg(target_os = "linux")]
    {
        match Command::new("lsblk").args(["--json", "--output", "PATH,TYPE,FSTYPE,LABEL,UUID,MOUNTPOINTS"]).output() {
            Ok(out) if out.status.success() => match serde_json::from_slice::<Value>(&out.stdout) {
                Ok(v) => return (parse_lsblk(&v), Vec::new()),
                Err(_) => return (Vec::new(), vec!["Could not parse lsblk output; automatic partition discovery is unavailable. Add readable paths manually.".into()]),
            },
            _ => return (Vec::new(), vec!["lsblk is missing or failed; automatic partition discovery is unavailable. Install util-linux or add readable paths manually.".into()]),
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

pub fn parse_lsblk(data: &Value) -> Vec<Volume> {
    fn visit(value: &Value, output: &mut Vec<Volume>) {
        let filesystem = value["fstype"].as_str().unwrap_or("");
        if ["ntfs", "btrfs", "ext4", "crypto_LUKS", "BitLocker"]
            .iter()
            .any(|s| s.eq_ignore_ascii_case(filesystem))
        {
            let mounts: Vec<String> = value["mountpoints"]
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
                .find(|os| *os != Os::Unknown)
                .unwrap_or(Os::Unknown);
            let note = if mounts.is_empty() {
                unreadable_note(Os::Linux, filesystem, false)
            } else {
                "Readable mount points will be checked for Steam libraries.".into()
            };
            output.push(Volume {
                id: value["uuid"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| value["path"].as_str().unwrap_or("unknown"))
                    .into(),
                device: value["path"].as_str().unwrap_or("unknown").into(),
                filesystem: filesystem.into(),
                mountpoints: mounts,
                os,
                note,
            });
        }
        for child in value["children"].as_array().into_iter().flatten() {
            visit(child, output);
        }
    }
    let mut result = Vec::new();
    for v in data["blockdevices"].as_array().into_iter().flatten() {
        visit(v, &mut result);
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
$rows = @(Get-Partition | ForEach-Object {
    $p = $_; $v = $p | Get-Volume -ErrorAction SilentlyContinue
    $linux = "$($p.GptType)" -eq '{0fc63daf-8483-4772-8e79-3d69d8477de4}' -or $p.MbrType -eq 131
    if ($v -or $linux) {
        [PSCustomObject]@{
            id = "$($p.DiskNumber):$($p.PartitionNumber):$($p.Guid)"
            disk = $p.DiskNumber; offset = $p.Offset
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
            device: format!("Disk {disk}, offset {offset}"),
            filesystem,
            mountpoints: mounts,
            os,
            note,
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
    #[test]
    fn unmounted_partition_is_not_a_confirmed_windows_install() {
        let data = serde_json::json!({"blockdevices":[{"path":"/dev/test", "children":[
            {"path":"/dev/test1","fstype":"ntfs","uuid":"abc","mountpoints":[null]},
            {"path":"/dev/test2","fstype":"swap","mountpoints":["[SWAP]"]}]}]});
        let volumes = parse_lsblk(&data);
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].os, Os::Unknown);
        assert!(volumes[0].note.contains("not yet verified"));
    }
    #[test]
    fn missing_reader_has_actionable_message() {
        assert!(unreadable_note(Os::Windows, "btrfs", false).contains("WinBtrfs was not detected"));
        assert!(unreadable_note(Os::Windows, "btrfs", true).contains("installed"));
        assert!(unreadable_note(Os::Windows, "ext4", false).contains("reader/driver"));
    }
}

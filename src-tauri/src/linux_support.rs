use crate::{discovery, model::*, platform};
use std::{collections::BTreeSet, fs, path::Path};

const CLIENTS: [&str; 3] = ["ferestre", "xodus-nexus", "xodus-nexus-launcher"];

pub fn scan(report: &Report) -> Vec<LinuxSystem> {
    let homes = discovery::homes(report);
    let mut systems = Vec::new();
    let mut seen = BTreeSet::new();
    if report.host_os == Os::Linux {
        let local_homes = homes
            .iter()
            .filter(|h| h.local && h.os == Os::Linux)
            .map(|h| h.path.as_path())
            .collect::<Vec<_>>();
        let runtime = CLIENTS
            .iter()
            .any(|name| discovery::program(name).is_some())
            || discovery::desktop_command("ferestre").is_some()
            || discovery::desktop_command("xodus").is_some()
            || installed(Path::new("/"), &local_homes);
        systems.push(LinuxSystem {
            root: "/".into(),
            xbox_runtime: runtime,
        });
        seen.insert(Path::new("/").canonicalize().unwrap_or_default());
    }
    for mount in discovery::roots(report) {
        for suffix in ["", "@", "@root", "rootfs"] {
            let root = if suffix.is_empty() {
                mount.path.clone()
            } else {
                mount.path.join(suffix)
            };
            if platform::os_at(&root) != Os::Linux || fs::read_dir(&root).is_err() {
                continue;
            }
            let Ok(identity) = root.canonicalize() else {
                continue;
            };
            if !seen.insert(identity) {
                continue;
            }
            let foreign_homes = homes
                .iter()
                .filter(|h| !h.local && h.os == Os::Linux && h.path.starts_with(&mount.path))
                .map(|h| h.path.as_path())
                .collect::<Vec<_>>();
            systems.push(LinuxSystem {
                root: root.to_string_lossy().into_owned(),
                xbox_runtime: installed(&root, &foreign_homes),
            });
        }
    }
    systems
}

fn installed(root: &Path, homes: &[&Path]) -> bool {
    for folder in [
        "usr/bin",
        "usr/local/bin",
        "opt/ferestre",
        "opt/xodus-nexus",
    ] {
        if CLIENTS
            .iter()
            .any(|name| resolve_file(root, &root.join(folder).join(name)).is_some())
        {
            return true;
        }
    }
    if [
        "usr/share/applications",
        "var/lib/flatpak/exports/share/applications",
    ]
    .iter()
    .any(|p| desktops(root, &root.join(p)))
    {
        return true;
    }
    homes.iter().any(|home| {
        [".local/bin", ".cargo/bin"].iter().any(|p| {
            CLIENTS
                .iter()
                .any(|name| resolve_file(root, &home.join(p).join(name)).is_some())
        }) || [
            ".local/share/applications",
            ".local/share/flatpak/exports/share/applications",
        ]
        .iter()
        .any(|p| desktops(root, &home.join(p)))
            || [
                ".config/xodus-nexus",
                ".config/Xodus Nexus",
                ".config/xodus-nexus-launcher",
            ]
            .iter()
            .any(|p| {
                ["launch-game", "launch-nexus"].iter().any(|name| {
                    resolve_file(root, &home.join(p).join("shortcuts").join(name)).is_some()
                })
            })
    })
}

fn resolve_file(root: &Path, path: &Path) -> Option<std::path::PathBuf> {
    let mut path = path.to_owned();
    for _ in 0..8 {
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.is_file() {
            return Some(path);
        }
        if !metadata.is_symlink() {
            return None;
        }
        let target = fs::read_link(&path).ok()?;
        path = if target.has_root() {
            root.join(target.strip_prefix("/").ok()?)
        } else {
            path.parent()?.join(target)
        };
    }
    None
}

fn desktops(root: &Path, folder: &Path) -> bool {
    fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(2048)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".desktop") || !(name.contains("ferestre") || name.contains("xodus"))
            {
                return false;
            }
            resolve_file(root, &entry.path())
                .and_then(|p| fs::read_to_string(p).ok())
                .is_some_and(|text| {
                    !text.lines().any(|line| line.trim() == "Hidden=true")
                        && text.lines().any(|line| {
                            line.strip_prefix("Exec=")
                                .is_some_and(|s| !s.trim().is_empty())
                        })
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_can_inspect_separate_linux_roots_including_btrfs_subvolumes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("linux-a");
        let b = dir.path().join("linux-b");
        for root in [&a, &b.join("@")] {
            fs::create_dir_all(root.join("etc")).unwrap();
            fs::write(root.join("etc/os-release"), "ID=fixture").unwrap();
        }
        fs::create_dir_all(a.join("usr/bin")).unwrap();
        fs::write(a.join("usr/bin/ferestre"), "runtime").unwrap();
        let mut report = crate::steam::scan_sources(
            &[],
            &[],
            &[],
            &OwnedLibrary::default(),
            None,
            vec![],
            vec![],
        );
        report.host_os = Os::Windows;
        report.volumes = [&a, &b]
            .into_iter()
            .map(|root| Volume {
                id: root.to_string_lossy().into(),
                device: String::new(),
                filesystem: "btrfs".into(),
                mountpoints: vec![root.to_string_lossy().into()],
                os: Os::Linux,
                note: String::new(),
            })
            .collect();
        let systems = scan(&report);
        assert_eq!(systems.len(), 2);
        assert!(
            systems
                .iter()
                .find(|s| s.root == a.to_string_lossy())
                .unwrap()
                .xbox_runtime
        );
        assert!(
            !systems
                .iter()
                .find(|s| s.root == b.join("@").to_string_lossy())
                .unwrap()
                .xbox_runtime
        );
    }
    #[test]
    fn runtimes_are_detected_without_games_and_scoped_to_each_installation() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("linux-a");
        let b = dir.path().join("linux-b");
        fs::create_dir_all(a.join("usr/bin")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("usr/bin/ferestre"), "runtime").unwrap();
        assert!(installed(&a, &[]));
        assert!(!installed(&b, &[]));
        let home = b.join("home/user");
        fs::create_dir_all(home.join(".local/state/ferestre")).unwrap();
        fs::write(home.join(".local/state/ferestre/library.json"), "{}").unwrap();
        assert!(!installed(&b, &[&home]));
        fs::create_dir_all(home.join(".local/share/applications")).unwrap();
        fs::write(
            home.join(".local/share/applications/xodus-nexus.desktop"),
            "[Desktop Entry]\nExec=/opt/xodus-nexus\n",
        )
        .unwrap();
        assert!(installed(&b, &[&home]));
    }
}

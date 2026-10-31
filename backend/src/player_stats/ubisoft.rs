use super::*;
use prost::Message;
use std::{collections::BTreeMap, io::Cursor};

#[derive(Clone, PartialEq, Message)]
struct Spool {
    #[prost(message, repeated, tag = "1")]
    records: Vec<Unlock>,
}
#[derive(Clone, PartialEq, Message)]
struct Unlock {
    #[prost(uint32, tag = "1")]
    id: u32,
    #[prost(uint64, tag = "2")]
    time: u64,
}

pub(super) fn cached(game: &LibraryGame, report: &Report, cache: &Path) -> PlayerStats {
    if !game.id.bytes().all(|c| c.is_ascii_digit()) {
        return PlayerStats::default();
    }
    let homes = super::local::homes(report);
    let spool = homes
        .iter()
        .flat_map(|h| {
            let root = h.join("AppData/Local/Ubisoft Game Launcher/spool");
            let mut dirs = crate::discovery::children(&root);
            dirs.push(root);
            dirs
        })
        .map(|p| p.join(format!("{}.spool", game.id)))
        .filter(|p| p.is_file())
        .max_by_key(|p| fs::metadata(p).ok().and_then(|m| m.modified().ok()));
    let Some(spool) = spool else {
        return PlayerStats::default();
    };
    let Some(unlocks) = read(&spool, 1024 * 1024).and_then(|b| Spool::decode(b.as_slice()).ok())
    else {
        return PlayerStats::default();
    };
    let mut roots: Vec<PathBuf> = crate::discovery::roots(report)
        .into_iter()
        .map(|r| r.path)
        .collect();
    roots.extend(
        crate::discovery::prefixes(report)
            .into_iter()
            .map(|p| p.path.join("drive_c")),
    );
    let mut archives: Vec<_> = roots
        .iter()
        .flat_map(|r| {
            [
                r.join("Program Files (x86)/Ubisoft/Ubisoft Game Launcher/cache/achievements"),
                r.join("Program Files/Ubisoft/Ubisoft Game Launcher/cache/achievements"),
                r.join("ProgramData/Ubisoft/Ubisoft Game Launcher/cache/achievements"),
            ]
        })
        .collect();
    for home in &homes {
        archives.push(home.join("AppData/Local/Ubisoft Game Launcher/cache/achievements"));
    }
    if let Some(config) = Path::new(&game.source).parent().and_then(|p| p.parent()) {
        archives.push(config.join("achievements"));
    }
    let prefix = format!("{}_", game.id);
    for dir in archives {
        let paths: Vec<_> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(2048)
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect();
        for path in paths {
            if let Some(achievements) = archive(&path, &unlocks, cache, &game.id) {
                return PlayerStats {
                    account: spool.parent().unwrap().to_string_lossy().into(),
                    source: "Ubisoft Connect cache".into(),
                    total: Some(achievements.len()),
                    achievements,
                    ..Default::default()
                };
            }
        }
    }
    PlayerStats::default()
}
fn archive(path: &Path, spool: &Spool, root: &Path, id: &str) -> Option<Vec<Achievement>> {
    let data = read(path, 32 * 1024 * 1024)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).ok()?;
    let mut texts = BTreeMap::new();
    let mut names: Vec<_> = zip
        .file_names()
        .filter_map(Result::ok)
        .filter(|n| n.to_ascii_lowercase().ends_with("_loc.txt"))
        .map(|n| n.into_owned())
        .collect();
    names.sort_by_key(|n| {
        if n.to_ascii_lowercase().starts_with("en-us") {
            0
        } else if n.to_ascii_lowercase().starts_with("en") {
            1
        } else {
            2
        }
    });
    for name in names {
        let mut text = String::new();
        zip.by_name(&name)
            .ok()?
            .take(2 * 1024 * 1024)
            .read_to_string(&mut text)
            .ok()?;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let fields: Vec<_> = line.splitn(3, '\t').collect();
            if fields.len() == 3
                && let Ok(id) = fields[0].parse::<u32>()
            {
                texts.insert(id, (fields[1].to_owned(), fields[2].to_owned()));
            }
        }
        if !texts.is_empty() {
            break;
        }
    }
    if texts.is_empty() {
        return None;
    }
    let icons = root.join("ubisoft-icons").join(id);
    let _ = fs::create_dir_all(&icons);
    Some(
        texts
            .into_iter()
            .map(|(id, (name, description))| {
                let path = icons.join(format!("{id}.png"));
                if !path.is_file()
                    && let Ok(file) = zip.by_name(&format!("{id}.png"))
                {
                    let mut bytes = Vec::new();
                    if file.take(1024 * 1024 + 1).read_to_end(&mut bytes).is_ok()
                        && bytes.len() <= 1024 * 1024
                    {
                        let _ = fs::write(&path, bytes);
                    }
                }
                let unlocked_at = spool
                    .records
                    .iter()
                    .filter(|r| r.id == id && r.time > 0)
                    .map(|r| {
                        if r.time > 10_000_000_000 {
                            r.time / 1000
                        } else {
                            r.time
                        }
                    })
                    .min();
                Achievement {
                    id: id.to_string(),
                    name,
                    description,
                    icon: if path.is_file() {
                        path.to_string_lossy().into()
                    } else {
                        String::new()
                    },
                    unlocked: unlocked_at.is_some(),
                    unlocked_at,
                    ..Default::default()
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn cached_archive_joins_localized_definitions_to_unlock_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("42_spec");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("en-US_loc.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"1\tFirst\tWin once\n2\tSecond\tWin twice")
            .unwrap();
        zip.finish().unwrap();
        let a = archive(
            &path,
            &Spool {
                records: vec![Unlock {
                    id: 1,
                    time: 1700000000000,
                }],
            },
            dir.path(),
            "42",
        )
        .unwrap();
        assert_eq!(a.len(), 2);
        assert!(a[0].unlocked);
        assert_eq!(a[0].unlocked_at, Some(1700000000));
        assert!(!a[1].unlocked);
        assert_eq!(a[0].rarity, None);
    }
}

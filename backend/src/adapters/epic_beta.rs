use super::*;
use crate::{discovery, epic, steam};
use std::{collections::BTreeSet, path::PathBuf};

pub fn scan(scan: &mut Scan<'_>) {
    let mut profiles = Vec::new();
    for home in &scan.homes {
        for suffix in [
            ".local/share/com.epicgames.eglv2",
            ".config/com.epicgames.eglv2",
            "AppData/Local/com.epicgames.eglv2",
        ] {
            profiles.push(Location {
                path: home.path.join(suffix),
                ..home.clone()
            });
        }
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        profiles.push(Location {
            path: PathBuf::from(data).join("com.epicgames.eglv2"),
            os: Os::host(),
            local: true,
            client: "Epic Preview".into(),
            drive_c: None,
        });
    }
    for prefix in &scan.prefixes {
        for user in discovery::children(&prefix.path.join("drive_c/users")) {
            profiles.push(Location {
                path: user.join("AppData/Local/com.epicgames.eglv2"),
                ..prefix.clone()
            });
        }
    }
    let mut seen = BTreeSet::new();
    for profile in profiles {
        if !seen.insert(epic::key(&profile.path)) {
            continue;
        }
        let path = profile.path.join("installations.jsonl");
        file(scan, &path, |scan| read(scan, &profile));
    }
}

fn read(scan: &mut Scan<'_>, profile: &Location) -> Result<(), String> {
    let source = epic::key(&profile.path.join("installations.jsonl"));
    let cache =
        discovery::json(&profile.path.join("cache/prod/catalog-cache.json")).unwrap_or_default();
    let content = steam::read_text(&profile.path.join("installations.jsonl"))?;
    let mut expected = None;
    let mut records = 0;
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line)
            .map_err(|_| "Incomplete Epic Preview installation journal")?;
        if row.get("entries").is_some() && row.get("installationId").is_none() {
            expected = row.get("entries").and_then(|v| v.as_u64());
            continue;
        }
        records += 1;
        // The Preview aggregates Steam/Xbox too. Only its own artifact records belong here.
        let Some(id) = text(&row, "artifactId") else {
            continue;
        };
        let Some(namespace) = text(&row, "namespace") else {
            continue;
        };
        if ["ue", "steam", "xbox"].contains(&namespace.to_ascii_lowercase().as_str())
            || text(&row, "provider").is_some_and(|p| !p.eq_ignore_ascii_case("epic"))
        {
            continue;
        }
        let item_id = text(&row, "itemId").ok_or("Epic Preview record has no catalog item")?;
        let meta = cache
            .get("items")
            .and_then(|v| v.get(item_id))
            .and_then(|v| v.get("item"));
        if meta.is_some_and(|m| !epic::is_game(m)) {
            let mut excluded = catalog::entry("epic", id, id, vec![], &source);
            excluded.is_game = Some(false);
            catalog::merge(&mut scan.games, excluded);
            continue;
        }
        let title = meta
            .and_then(|m| text(m, "title"))
            .or_else(|| text(&row, "displayName"))
            .unwrap_or(id);
        let raw = text(&row, "dir").ok_or("Epic Preview record has no directory")?;
        let path = discovery::resolve(raw, profile, scan.report)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(raw.into());
        let ready = row.get("complete").and_then(|v| v.as_bool()) == Some(true)
            && text(&row, "installationId").is_some_and(|id| {
                PathBuf::from(&path)
                    .join(".egstore")
                    .join(format!("{id}.manifest"))
                    .is_file()
            });
        let mut install = catalog::install(&source, path, profile.os, ready);
        if !profile.local && discovery::resolve(raw, profile, scan.report).is_none() {
            install.state = "unavailable".into();
        }
        let mut game = catalog::entry("epic", id, title, vec![install], &source);
        game.client = "Epic Preview".into();
        game.cover = meta.and_then(image);
        let app = format!(
            "{}%3A{}%3A{}",
            catalog::encode(namespace),
            catalog::encode(item_id),
            catalog::encode(id)
        );
        game.store_page = Some(catalog::url(
            "Epic Preview",
            format!(
                "https://store.epicgames.com/browse?q={}",
                catalog::encode(title)
            ),
        ));
        if let Some(slug) = meta.and_then(|m| text(m, "productSlug").or_else(|| text(m, "urlSlug")))
        {
            game.store_page = Some(catalog::url(
                "Epic",
                format!("https://store.epicgames.com/p/{}", catalog::encode(slug)),
            ));
        }
        if catalog::local(&game.installations) {
            let uri = format!("com.epicgames.launcher://apps/{app}?action=launch&silent=true");
            if profile.drive_c.is_none() || cfg!(windows) {
                game.launch = Some(catalog::url("Epic Preview", uri));
                let mut action = catalog::url(
                    "Epic Preview",
                    "com.epicgames.launcher://store/library".into(),
                );
                action.note =
                    Some("Select this game in Epic's library to finish uninstalling.".into());
                game.uninstall = Some(action);
            }
        }
        catalog::merge(&mut scan.games, game);
    }
    if expected.is_some_and(|n| n != records) {
        return Err("Epic Preview is rewriting its journal; previous records retained".into());
    }
    Ok(())
}

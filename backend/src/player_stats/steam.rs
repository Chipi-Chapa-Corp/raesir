use super::*;
use crate::{model::Os, platform, vdf};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn cached(game: &LibraryGame, report: &Report) -> PlayerStats {
    let mut roots = platform::local_sources();
    roots.extend(report.libraries.clone());
    roots.sort_by_key(|r| r.os != Os::host());
    let mut seen = BTreeSet::new();
    let mut stats = PlayerStats::default();
    for root in roots {
        let mut root = PathBuf::from(root.path);
        if root
            .file_name()
            .is_some_and(|s| s.eq_ignore_ascii_case("steamapps"))
        {
            root.pop();
        }
        if !seen.insert(root.canonicalize().unwrap_or(root.clone())) {
            continue;
        }
        let Some(users) = crate::steam::read_text(&root.join("config/loginusers.vdf"))
            .ok()
            .and_then(|s| vdf::parse(&s).ok())
        else {
            continue;
        };
        let Some(users) = users.get("users").and_then(|v| v.entries()) else {
            continue;
        };
        let selected = if let Some(id) = report.owned_account.as_deref() {
            users.iter().find(|(key, _)| key == id)
        } else {
            users
                .iter()
                .find(|(_, v)| v.field("MostRecent") == Some("1"))
        };
        let Some((id, _)) = selected else {
            continue;
        };
        let Some(uid) = id
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_sub(76561197960265728))
        else {
            continue;
        };
        if !stats.account.is_empty() && stats.account != *id {
            continue;
        }
        stats.account = id.clone();
        stats.source = "Steam".into();
        let config = root.join(format!("userdata/{uid}/config"));
        if let Some(v) = crate::steam::read_text(&config.join("localconfig.vdf"))
            .ok()
            .and_then(|s| vdf::parse(&s).ok())
        {
            let app = [
                "UserLocalConfigStore",
                "Software",
                "Valve",
                "Steam",
                "apps",
                &game.id,
            ]
            .into_iter()
            .try_fold(&v, |v, k| v.get(k));
            if let Some(minutes) = app
                .and_then(|v| v.field("Playtime"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                stats.playtime_seconds = Some(
                    stats
                        .playtime_seconds
                        .unwrap_or(0)
                        .max(minutes.saturating_mul(60)),
                );
            }
        }
        if let Some(v) = json(&config.join(format!("librarycache/{}.json", game.id))) {
            let found = parse_cache(&v);
            if found.achievements.len() > stats.achievements.len() {
                stats.achievements = found.achievements;
                stats.total = found.total;
                stats.unlocked_count = found.unlocked_count;
                stats.partial = found.partial;
            }
        }
    }
    stats
}

pub(super) fn apply_hidden(game: &LibraryGame, report: &Report, stats: &mut PlayerStats) {
    let mut roots = platform::local_sources();
    roots.extend(report.libraries.clone());
    let mut seen = BTreeSet::new();
    for source in roots {
        let mut root = PathBuf::from(source.path);
        if root
            .file_name()
            .is_some_and(|s| s.eq_ignore_ascii_case("steamapps"))
        {
            root.pop();
        }
        let path = root.join(format!(
            "appcache/stats/UserGameStatsSchema_{}.bin",
            game.id
        ));
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(data) = read(&path, 8 * 1024 * 1024) else {
            continue;
        };
        let hidden = hidden_ids(&data);
        for achievement in &mut stats.achievements {
            achievement.hidden |= hidden.contains(&achievement.id.to_ascii_lowercase());
        }
    }
}
fn hidden_ids(data: &[u8]) -> BTreeSet<String> {
    fn visit(value: &steam_vdf_parser::Value<'_>, ids: &mut BTreeSet<String>, depth: usize) {
        if depth > 24 {
            return;
        }
        let Some(object) = value.as_obj() else {
            return;
        };
        if let Some(name) = object.get("name").and_then(|v| v.as_str()) {
            if let Some(hidden) = object.get("display").and_then(|v| v.get("hidden")) {
                if hidden.as_str() == Some("1") || hidden.as_i32() == Some(1) {
                    ids.insert(name.to_ascii_lowercase());
                }
            }
        }
        for (_, child) in object.iter() {
            visit(child, ids, depth + 1);
        }
    }
    let mut ids = BTreeSet::new();
    if let Ok(schema) = steam_vdf_parser::parse_shortcuts(data) {
        if let Some(root) = schema.as_obj() {
            for (_, value) in root.iter() {
                visit(value, &mut ids, 0);
            }
        }
    }
    ids
}

fn parse_cache(value: &Value) -> PlayerStats {
    let data = value
        .as_array()
        .into_iter()
        .flatten()
        .find(|pair| pair[0] == "achievements")
        .map(|pair| &pair[1]["data"]);
    let Some(data) = data else {
        return PlayerStats::default();
    };
    let mut achievements = Vec::new();
    let mut ids = BTreeSet::new();
    for key in ["vecHighlight", "vecUnachieved", "vecAchievedHidden"] {
        for a in data[key].as_array().into_iter().flatten() {
            let id = a["strID"].as_str().unwrap_or("");
            if id.is_empty() || !ids.insert(id.to_owned()) {
                continue;
            }
            let min = a["flMinProgress"].as_f64().unwrap_or(0.);
            let max = a["flMaxProgress"].as_f64().unwrap_or(0.);
            achievements.push(Achievement {
                id: id.into(),
                name: a["strName"].as_str().unwrap_or("Hidden achievement").into(),
                description: a["strDescription"].as_str().unwrap_or("").into(),
                icon: a["strImage"].as_str().unwrap_or("").into(),
                unlocked: a["bAchieved"].as_bool().unwrap_or(false),
                hidden: a["bHidden"].as_bool().unwrap_or(key == "vecAchievedHidden"),
                unlocked_at: a["rtUnlocked"].as_u64().filter(|n| *n > 0),
                progress: a["flCurrentProgress"]
                    .as_f64()
                    .filter(|_| max > min)
                    .map(|n| ((n - min) / (max - min)).clamp(0., 1.)),
                rarity: a["flAchieved"]
                    .as_f64()
                    .filter(|n| (0. ..=100.).contains(n)),
            });
        }
    }
    let total = data["nTotal"].as_u64().map(|n| n as usize);
    PlayerStats {
        partial: total.is_some_and(|n| n > achievements.len()),
        total,
        unlocked_count: data["nAchieved"].as_u64().map(|n| n as usize),
        achievements,
        ..Default::default()
    }
}

pub(super) fn refresh(game: &LibraryGame, stats: &mut PlayerStats) -> bool {
    if stats.account.len() != 17
        || !stats.account.bytes().all(|b| b.is_ascii_digit())
        || !game.id.bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }
    let Some(client) = client() else {
        return false;
    };
    let Some(mut response) = client
        .get(format!(
            "https://steamcommunity.com/profiles/{}/stats/{}/?xml=1&l=english",
            stats.account, game.id
        ))
        .send()
        .ok()
        .and_then(|r| r.error_for_status().ok())
    else {
        return false;
    };
    let mut bytes = String::new();
    if response
        .by_ref()
        .take(4 * 1024 * 1024 + 1)
        .read_to_string(&mut bytes)
        .is_err()
        || bytes.len() > 4 * 1024 * 1024
    {
        return false;
    }
    let Some(mut achievements) = parse_xml(&bytes) else {
        return false;
    };
    for a in &mut achievements {
        if let Some(old) = stats
            .achievements
            .iter()
            .find(|old| old.id.eq_ignore_ascii_case(&a.id))
        {
            a.rarity = old.rarity;
            a.hidden |= old.hidden;
            a.progress = old.progress;
        }
    }
    // Rarity is public even when account details are private.
    if let Some(v) = client.get(format!("https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid={}",game.id)).send().ok().and_then(response_json) {
        apply_rarity(&mut achievements,&v);
    }
    stats.total = Some(achievements.len());
    stats.unlocked_count = Some(achievements.iter().filter(|a| a.unlocked).count());
    stats.achievements = achievements;
    stats.partial = false;
    true
}
fn response_json(r: reqwest::blocking::Response) -> Option<Value> {
    super::response(r)
}
fn apply_rarity(achievements: &mut [Achievement], value: &Value) {
    for item in value["achievementpercentages"]["achievements"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(name) = item["name"].as_str() else {
            continue;
        };
        let Some(percent) = number(&item["percent"]).filter(|n| (0. ..=100.).contains(n)) else {
            continue;
        };
        if let Some(a) = achievements
            .iter_mut()
            .find(|a| a.id.eq_ignore_ascii_case(name))
        {
            a.rarity = Some(percent);
        }
    }
}
fn parse_xml(text: &str) -> Option<Vec<Achievement>> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let root = doc.descendants().find(|n| n.has_tag_name("achievements"))?;
    Some(
        root.children()
            .filter(|n| n.has_tag_name("achievement"))
            .filter_map(|a| {
                let field = |key| {
                    a.children()
                        .find(|n| n.has_tag_name(key))
                        .and_then(|n| n.text())
                        .unwrap_or("")
                        .to_owned()
                };
                let id = field("apiname");
                if id.is_empty() {
                    return None;
                }
                let unlocked = a.attribute("closed") == Some("1");
                Some(Achievement {
                    id,
                    name: field("name"),
                    description: field("description"),
                    icon: field(if unlocked { "iconClosed" } else { "iconOpen" }),
                    unlocked,
                    hidden: field("hidden") == "1"
                        || field("name").eq_ignore_ascii_case("Hidden achievement"),
                    unlocked_at: field("unlockTimestamp")
                        .parse::<u64>()
                        .ok()
                        .filter(|n| *n > 0),
                    ..Default::default()
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_binary_schema_supplies_secret_flags_missing_from_community_xml() {
        let data = b"\x00root\x00\x00bits\x00\x001\x00\x01name\x00SECRET\x00\x00display\x00\x01hidden\x001\x00\x08\x08\x08\x08\x08";
        assert!(hidden_ids(data).contains("secret"));
        assert!(hidden_ids(b"invalid data").is_empty());
    }
    #[test]
    fn steam_reads_only_the_selected_account_and_keeps_minutes_as_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let id = "76561197960265770";
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/loginusers.vdf"),
            format!("users {{ {id} {{ MostRecent 0 }} 76561197960265771 {{ MostRecent 1 }} }}"),
        )
        .unwrap();
        let config = dir.path().join("userdata/42/config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("localconfig.vdf"),
            "UserLocalConfigStore { Software { Valve { Steam { apps { 1 { Playtime 90 } } } } } }",
        )
        .unwrap();
        let mut report:Report=serde_json::from_value(serde_json::json!({"host_os":"linux","games":[],"libraries":[{"path":dir.path(),"os":"linux"}],"volumes":[],"warnings":[],"owned_account":id})).unwrap();
        let game = crate::catalog::entry("steam", "1", "", vec![], "");
        let stats = cached(&game, &report);
        assert_eq!(stats.account, id);
        assert_eq!(stats.playtime_seconds, Some(5400));
        let saved = PlayerStats {
            account: id.into(),
            updated: details::now(),
            total: Some(1),
            achievements: vec![Achievement {
                id: "biome_excavation_site".into(),
                name: "Reached Coal Pits".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        fs::write(
            cache_path_version(&game, id, dir.path(), 1),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        let migrated = crate::player_stats::cached(&game, &report, dir.path());
        assert_eq!(migrated.achievements.len(), 1);
        assert_eq!(
            migrated.updated, 0,
            "legacy data must be displayed but refreshed"
        );
        let mut repaired = saved;
        repaired.achievements[0].rarity = Some(72.5);
        fs::write(
            cache_path(&game, id, dir.path()),
            serde_json::to_vec(&repaired).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(config.join("librarycache")).unwrap();
        fs::write(config.join("librarycache/1.json"),serde_json::json!([["achievements",{"data":{"nTotal":1,"vecUnachieved":[{"strID":"BIOME_EXCAVATION_SITE","strName":"Reached Coal Pits"}]}}]]).to_string()).unwrap();
        let complete = crate::player_stats::cached(&game, &report, dir.path());
        assert_eq!(
            complete.achievements[0].rarity,
            Some(72.5),
            "complete local caches still need online rarity enrichment"
        );
        assert_eq!(complete.updated, repaired.updated);
        report.owned_account = Some("76561197960265772".into());
        let stats = cached(&game, &report);
        assert!(stats.account.is_empty());
        assert!(!stats.has_data());
    }
    #[test]
    fn global_percentages_match_steam_ids_regardless_of_case() {
        let mut achievements = vec![
            Achievement {
                id: "biome_excavation_site".into(),
                ..Default::default()
            },
            Achievement {
                id: "BEAT_THE_GAME".into(),
                ..Default::default()
            },
            Achievement {
                id: "unknown".into(),
                ..Default::default()
            },
        ];
        apply_rarity(
            &mut achievements,
            &serde_json::json!({"achievementpercentages":{"achievements":[{"name":"BIOME_EXCAVATION_SITE","percent":"72.5"},{"name":"beat_the_game","percent":10.0},{"name":"unknown","percent":"invalid"}]}}),
        );
        assert_eq!(achievements[0].rarity, Some(72.5));
        assert_eq!(achievements[1].rarity, Some(10.));
        assert_eq!(achievements[2].rarity, None);
    }
    #[test]
    fn steam_cache_is_partial_and_xml_supplies_the_full_set() {
        let v = serde_json::json!([["achievements",{"data":{"nTotal":3,"vecHighlight":[{"strID":"a","strName":"A","bAchieved":true,"flAchieved":1.2}],"vecUnachieved":[{"strID":"b","strName":"B","flMinProgress":0,"flMaxProgress":10,"flCurrentProgress":6}],"vecAchievedHidden":[{"strID":"a"}]}}]]);
        let s = parse_cache(&v);
        assert!(s.partial);
        assert_eq!(s.achievements.len(), 2);
        assert_eq!(s.achievements[1].progress, Some(0.6));
        assert!(parse_xml("<response><error>Private</error></response>").is_none());
        let a=parse_xml("<playerstats><achievements><achievement closed=\"1\"><apiname>A</apiname><name>Win</name><unlockTimestamp>100</unlockTimestamp></achievement></achievements></playerstats>").unwrap();
        assert!(a[0].unlocked);
        assert_eq!(a[0].unlocked_at, Some(100));
    }
}

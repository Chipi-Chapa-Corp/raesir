use super::*;
use crate::discovery;
use serde_json::Value;

pub(super) fn homes(report: &Report) -> Vec<PathBuf> {
    let mut homes: Vec<_> = discovery::homes(report)
        .into_iter()
        .map(|h| h.path)
        .collect();
    for prefix in discovery::prefixes(report) {
        homes.extend(discovery::children(&prefix.path.join("drive_c/users")));
    }
    homes.sort();
    homes.dedup();
    homes
}
pub(super) fn playtime(game: &LibraryGame, report: &Report) -> Option<u64> {
    let mut best = None;
    for home in discovery::homes(report) {
        for suffix in [
            ".config/heroic",
            ".var/app/com.heroicgameslauncher.hgl/config/heroic",
            "AppData/Roaming/heroic",
        ] {
            if game.store != "epic" {
                continue;
            }
            if let Some(v) = json(&home.path.join(suffix).join("store/timestamp.json")) {
                if let Some(minutes) = v
                    .get(&game.id)
                    .and_then(|v| v["totalPlayed"].as_f64())
                    .filter(|n| n.is_finite() && *n >= 0.)
                {
                    best = Some(best.unwrap_or(0).max((minutes * 60.) as u64));
                }
            }
        }
        for suffix in [
            ".local/share/lutris/pga.db",
            ".var/app/net.lutris.Lutris/data/lutris/pga.db",
        ] {
            let path = home.path.join(suffix);
            if !path.is_file() {
                continue;
            }
            if let Ok(db) = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            ) {
                if let Some(seconds) = lutris(&db, game) {
                    best = Some(best.unwrap_or(0).max(seconds));
                }
            }
        }
    }
    best
}
fn lutris(db: &rusqlite::Connection, game: &LibraryGame) -> Option<u64> {
    let services = match game.store.as_str() {
        "steam" => ["steam", "steam"],
        "epic" => ["epic", "epic"],
        "ubisoft" => ["uplay", "ubisoft"],
        "ea" => ["origin", "ea_app"],
        _ => return None,
    };
    let hours = db
        .query_row(
            "SELECT MAX(playtime) FROM games WHERE service IN (?1, ?2) AND service_id = ?3",
            rusqlite::params![services[0], services[1], game.id],
            |r| r.get::<_, Option<f64>>(0),
        )
        .ok()??;
    (hours.is_finite() && hours >= 0.).then_some((hours * 3600.) as u64)
}
pub(super) fn ea(game: &LibraryGame, report: &Report) -> PlayerStats {
    let mut result = PlayerStats::default();
    for home in homes(report) {
        for name in ["EADesktopVerbose.bak", "EADesktopVerbose.log"] {
            let path = home
                .join("AppData/Local/Electronic Arts/EA Desktop/Logs")
                .join(name);
            let Some(bytes) = read(&path, 16 * 1024 * 1024) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let xml = ea_xml(&text, &game.id);
            if !xml.is_empty() {
                result.account = path.parent().unwrap_or(&home).to_string_lossy().into();
                result.source = "EA App cache".into();
                result.total = Some(xml.len());
                result.achievements = xml;
            }
            // Only structured GraphQL responses for this offer; unrelated log events are ignored.
            for line in String::from_utf8_lossy(&bytes)
                .lines()
                .filter(|l| l.contains(&game.id) && l.contains("achievements"))
            {
                let Some(start) = line.find('{') else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line[start..]) else {
                    continue;
                };
                let data = &v["data"]["achievements"];
                let Some(sets) = data.as_array() else {
                    continue;
                };
                for set in sets {
                    let Some(items) = set["achievements"].as_array() else {
                        continue;
                    };
                    result.account = path.parent().unwrap_or(&home).to_string_lossy().into();
                    result.source = "EA App cache".into();
                    result.achievements = items
                        .iter()
                        .filter_map(|a| {
                            Some(Achievement {
                                id: a["id"].as_str()?.into(),
                                name: a["name"].as_str()?.into(),
                                description: a["description"].as_str().unwrap_or("").into(),
                                hidden: a["hidden"].as_bool().unwrap_or(false),
                                icon: a["icon"]
                                    .as_str()
                                    .or_else(|| a["iconUrl"].as_str())
                                    .unwrap_or("")
                                    .into(),
                                unlocked: a["awardCount"].as_u64().unwrap_or(0) > 0,
                                unlocked_at: a["date"]
                                    .as_str()
                                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                    .map(|d| d.timestamp().max(0) as u64),
                                ..Default::default()
                            })
                        })
                        .collect();
                    result.total = Some(result.achievements.len());
                }
            }
        }
    }
    result
}

fn ea_xml(text: &str, offer: &str) -> Vec<Achievement> {
    let mut content_ids = std::collections::BTreeSet::new();
    if !offer.is_empty() && offer.bytes().all(|c| c.is_ascii_digit()) {
        content_ids.insert(offer.to_owned());
    }
    for line in text.lines().filter(|line| line.contains(offer)) {
        if let Some(start) = line.find('{') {
            let value = serde_json::Deserializer::from_str(&line[start..])
                .into_iter::<Value>()
                .next();
            if let Some(Ok(value)) = value {
                ea_content_ids(&value, offer, &mut content_ids);
            }
        }
    }
    let mut result = std::collections::BTreeMap::new();
    for fragment in text.split("<AchievementSet").skip(1) {
        let Some((body, _)) = fragment.split_once("</AchievementSet>") else {
            continue;
        };
        let xml = format!("<AchievementSet{body}</AchievementSet>");
        let Ok(doc) = roxmltree::Document::parse(&xml) else {
            continue;
        };
        let set = doc.root_element();
        let id = set.attribute("Name").unwrap_or("");
        if !id
            .split('_')
            .nth(1)
            .is_some_and(|id| content_ids.contains(id))
        {
            continue;
        }
        for node in set.children().filter(|n| n.has_tag_name("Achievement")) {
            let Some(id) = node.attribute("Id") else {
                continue;
            };
            let attr = |key| node.attribute(key).unwrap_or("");
            let num = |key| {
                attr(key)
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .unwrap_or(0.)
            };
            let total = num("Total");
            let unlocked = num("Count") > 0. || (total > 0. && num("Progress") >= total);
            let image_id = attr("ImageId").trim_start_matches("achieve:");
            let icon = if !image_id.is_empty()
                && image_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                format!(
                    "https://achievements.gameservices.ea.com/achievements/icons/{image_id}-208.png"
                )
            } else {
                String::new()
            };
            result.insert(
                format!("{}:{id}", set.attribute("Name").unwrap_or("")),
                Achievement {
                    id: format!("{}:{id}", set.attribute("Name").unwrap_or("")),
                    name: attr("Name").to_owned(),
                    description: attr("Description").to_owned(),
                    icon,
                    unlocked,
                    hidden: attr("Hidden").eq_ignore_ascii_case("true") || attr("Hidden") == "1",
                    unlocked_at: unlocked
                        .then(|| {
                            chrono::NaiveDateTime::parse_from_str(
                                attr("GrantDate"),
                                "%Y-%m-%dT%H:%M:%S",
                            )
                            .ok()
                            .map(|d| d.and_utc().timestamp().max(0) as u64)
                        })
                        .flatten()
                        .filter(|n| *n > 0),
                    progress: (total > 0.).then(|| (num("Progress") / total).clamp(0., 1.)),
                    ..Default::default()
                },
            );
        }
    }
    result.into_values().collect()
}
fn ea_content_ids(value: &Value, offer: &str, ids: &mut std::collections::BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if map.get("offerId").and_then(Value::as_str) == Some(offer) {
                if let Some(id) = map.get("masterTitleId").and_then(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                }) {
                    ids.insert(id);
                }
            }
            for value in map.values() {
                ea_content_ids(value, offer, ids);
            }
        }
        Value::Array(values) => {
            for value in values {
                ea_content_ids(value, offer, ids);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ea_xml_matches_offer_and_reads_unlocks_icons_and_progress() {
        let log = r#"log {"offerId":"OFR.1","masterTitleId":"42"}
<AchievementSet Name="1_42_1"><Achievement Id="1" Name="Winner" Description="Win once" ImageId="achieve:1_42_1-1" Count="1" GrantDate="2025-02-17T10:00:00"/><Achievement Id="2" Name="Secret" Description="Keep going" Progress="2" Total="10" Hidden="true"/></AchievementSet>
<AchievementSet Name="1_99_1"><Achievement Id="3" Name="Wrong game" Count="1"/></AchievementSet>"#;
        let stats = ea_xml(log, "OFR.1");
        assert_eq!(stats.len(), 2);
        assert!(stats[0].unlocked);
        assert!(stats[0].unlocked_at.is_some());
        assert!(stats[0].icon.ends_with("1_42_1-1-208.png"));
        assert!(stats[1].concealed());
        assert_eq!(stats[1].progress, Some(0.2));
        assert!(ea_xml(log, "different offer").is_empty());
    }
    #[test]
    fn lutris_uses_hours_and_matches_both_store_and_game_id() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE games(service TEXT, service_id TEXT, playtime REAL); INSERT INTO games VALUES ('epic','42',1.5),('steam','42',100),('epic','other',90);").unwrap();
        let game = crate::catalog::entry("epic", "42", "", vec![], "");
        assert_eq!(lutris(&db, &game), Some(5400));
        assert!(
            lutris(
                &db,
                &crate::catalog::entry("epic", "missing", "", vec![], "")
            )
            .is_none()
        );
    }
}

use super::*;
use serde_json::{Value, json};

fn profile(game: &LibraryGame, report: &Report) -> Option<(PathBuf, Value)> {
    let mut sources = crate::epic::local_sources();
    sources.extend(report.epic.sources.clone());
    sources.sort_by_key(|s| !s.local);
    sources
        .into_iter()
        .filter(|s| s.kind == "legendary")
        .find_map(|s| {
            let path = PathBuf::from(s.path);
            let user = super::json(&path.join("user.json"))?;
            user["account_id"].as_str()?;
            let owned = super::json(&path.join("assets.json")).is_some_and(|v| {
                v.as_object()
                    .into_iter()
                    .flat_map(|m| m.values())
                    .flat_map(|v| v.as_array().into_iter().flatten())
                    .any(|a| a["app_name"] == game.id)
            });
            owned.then_some((path, user))
        })
}
pub(super) fn cached(game: &LibraryGame, report: &Report) -> PlayerStats {
    let Some((_, user)) = profile(game, report) else {
        return PlayerStats::default();
    };
    PlayerStats {
        account: user["account_id"].as_str().unwrap_or("").into(),
        source: "Epic Games".into(),
        ..Default::default()
    }
}
pub(super) fn refresh(game: &LibraryGame, report: &Report, stats: &mut PlayerStats) -> bool {
    if game.id.contains(['/', '\\']) || game.id == ".." {
        return false;
    }
    let Some((path, user)) = profile(game, report) else {
        return false;
    };
    if user["account_id"].as_str() != Some(&stats.account) {
        return false;
    }
    let Some(token) = user["access_token"].as_str() else {
        return false;
    };
    let Some(client) = client() else {
        return false;
    };
    let mut success = false;
    let url = format!(
        "https://library-service.live.use1a.on.epicgames.com/library/api/public/playtime/account/{}/all",
        stats.account
    );
    if let Some(v) = client
        .get(url)
        .bearer_auth(token)
        .send()
        .ok()
        .and_then(response)
    {
        if let Some(time) = v
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["artifactId"] == game.id)
            .and_then(|a| a["totalTime"].as_u64())
        {
            stats.playtime_seconds = Some(time);
            success = true;
        }
    }
    let Some(meta) = super::json(&path.join(format!("metadata/{}.json", game.id))) else {
        return success;
    };
    let namespace = meta
        .pointer("/metadata/namespace")
        .or_else(|| meta.get("namespace"))
        .and_then(Value::as_str);
    let Some(namespace) = namespace else {
        return success;
    };
    let schema_query = "query Achievement($sandboxId:String!,$locale:String!){Achievement{productAchievementsRecordBySandbox(sandboxId:$sandboxId,locale:$locale){productId achievements{achievement{name unlockedDisplayName unlockedDescription unlockedIconLink lockedIconLink hidden rarity{percent}}}}}}";
    let schema = client
        .post("https://store.epicgames.com/graphql")
        .bearer_auth(token)
        .json(&json!({"query":schema_query,"variables":{"sandboxId":namespace,"locale":"en-US"}}))
        .send()
        .ok()
        .and_then(response);
    let Some(schema) = schema else {
        return success;
    };
    let data = &schema["data"]["Achievement"]["productAchievementsRecordBySandbox"];
    let Some(_) = data["productId"].as_str() else {
        return success;
    };
    let progress_query = "query PlayerAchievement($epicAccountId:String!,$sandboxId:String!){PlayerAchievement{playerAchievementGameRecordsBySandbox(epicAccountId:$epicAccountId,sandboxId:$sandboxId){records{playerAchievements{playerAchievement{achievementName unlocked progress unlockDate}}}}}}";
    let progress = client.post("https://store.epicgames.com/graphql").bearer_auth(token)
        .json(&json!({"query":progress_query,"variables":{"epicAccountId":stats.account,"sandboxId":namespace}}))
        .send().ok().and_then(response);
    let Some(progress) = progress else {
        return success;
    };
    let records = progress
        .pointer("/data/PlayerAchievement/playerAchievementGameRecordsBySandbox/records")
        .and_then(Value::as_array);
    let Some(records) = records else {
        return success;
    };
    let progress = Value::Array(
        records
            .iter()
            .flat_map(|r| {
                r["playerAchievements"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned()
            })
            .collect(),
    );
    stats.achievements = parse(data, &progress);
    stats.total = Some(stats.achievements.len());
    stats.partial = false;
    true
}
fn parse(schema: &Value, progress: &Value) -> Vec<Achievement> {
    schema["achievements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let a = &a["achievement"];
            let id = a["name"].as_str()?;
            let p = progress
                .as_array()?
                .iter()
                .map(|p| &p["playerAchievement"])
                .find(|p| p["achievementName"] == id);
            let unlocked = p.and_then(|p| p["unlocked"].as_bool()).unwrap_or(false);
            Some(Achievement {
                id: id.into(),
                name: a["unlockedDisplayName"]
                    .as_str()
                    .unwrap_or("Hidden achievement")
                    .into(),
                description: a["unlockedDescription"].as_str().unwrap_or("").into(),
                icon: a[if unlocked {
                    "unlockedIconLink"
                } else {
                    "lockedIconLink"
                }]
                .as_str()
                .unwrap_or("")
                .into(),
                unlocked,
                hidden: a["hidden"].as_bool().unwrap_or(false),
                unlocked_at: p
                    .and_then(|p| p["unlockDate"].as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.timestamp().max(0) as u64),
                progress: p
                    .and_then(|p| p["progress"].as_f64())
                    .map(|p| (p / 100.).clamp(0., 1.)),
                rarity: number(&a["rarity"]["percent"]).filter(|n| (0. ..=100.).contains(n)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epic_joins_schema_to_account_progress_by_id() {
        let a = parse(
            &json!({"achievements":[{"achievement":{"name":"x","unlockedDisplayName":"Win","rarity":{"percent":2.5}}}]}),
            &json!([{"playerAchievement":{"achievementName":"x","unlocked":false,"progress":75}}]),
        );
        assert_eq!(a[0].progress, Some(0.75));
        assert_eq!(a[0].rarity, Some(2.5));
        assert!(!a[0].unlocked);
    }
}

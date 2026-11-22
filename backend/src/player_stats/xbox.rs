use super::{Achievement, PlayerStats, number};
use crate::{ferestre::account, model::LibraryGame, xbox_store};
use serde_json::Value;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

pub(super) fn cached(game: &LibraryGame) -> PlayerStats {
    if !xbox_store::product_id(&game.id) {
        return PlayerStats::default();
    }
    let Some(profile) = account::player_profile() else {
        return PlayerStats::default();
    };
    PlayerStats {
        account: profile,
        source: "Xbox".into(),
        ..Default::default()
    }
}

pub(super) fn refresh(game: &LibraryGame, stats: &mut PlayerStats) -> bool {
    if stats.account.is_empty() {
        return false;
    }
    let Ok(client) = account::xodus_client() else {
        return false;
    };
    let Some(title_id) = xbox_store::achievement_title_id(&game.id) else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let result = fetch(title_id, |continuation| {
        let timeout = deadline.checked_duration_since(Instant::now())?;
        account::player_achievement_page(
            &client,
            &stats.account,
            title_id,
            continuation,
            timeout.min(Duration::from_secs(10)),
        )
    });
    let Some(achievements) = result else {
        return false;
    };
    stats.total = Some(achievements.len());
    stats.unlocked_count = Some(achievements.iter().filter(|a| a.unlocked).count());
    stats.achievements = achievements;
    stats.partial = false;
    true
}

fn fetch(
    title_id: u32,
    mut page: impl FnMut(Option<&str>) -> Option<Value>,
) -> Option<Vec<Achievement>> {
    let mut achievements = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_tokens = HashSet::new();
    let mut continuation: Option<String> = None;
    for _ in 0..10 {
        let value = page(continuation.as_deref())?;
        for row in value["achievements"].as_array()? {
            if row["isRevoked"] == true {
                continue;
            }
            if let Some(titles) = row["titleAssociations"].as_array()
                && !titles.iter().any(|title| {
                    title["id"].as_u64() == Some(u64::from(title_id))
                        || title["id"].as_str().and_then(|id| id.parse().ok()) == Some(title_id)
                })
            {
                continue;
            }
            let achievement = parse(row)?;
            if seen_ids.insert(achievement.id.clone()) {
                achievements.push(achievement);
            }
        }
        let token = &value["pagingInfo"]["continuationToken"];
        if token.is_null() || token.as_str() == Some("") {
            return Some(achievements);
        }
        let token = token.as_str()?.to_owned();
        if token.len() > 4096 || !seen_tokens.insert(token.clone()) {
            return None;
        }
        continuation = Some(token);
    }
    None
}

fn parse(row: &Value) -> Option<Achievement> {
    let id = row["id"].as_str().filter(|id| !id.is_empty())?;
    let id = match row["serviceConfigId"].as_str() {
        Some(scid) => format!("{scid}:{id}"),
        None => id.to_owned(),
    };
    let unlocked = row["progressState"]
        .as_str()
        .or_else(|| row.pointer("/progression/achievementState")?.as_str())?
        .eq_ignore_ascii_case("Achieved");
    let mut description = if unlocked {
        "description"
    } else {
        "lockedDescription"
    };
    if unlocked && row[description].as_str().is_none_or(str::is_empty) {
        description = "lockedDescription";
    }
    let icon = row["mediaAssets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|asset| asset["type"] == "Icon")
        .filter_map(|asset| asset["url"].as_str())
        .find(|url| url.starts_with("https://"))
        .unwrap_or_default()
        .to_owned();
    let unlocked_at = unlocked
        .then(|| row.pointer("/progression/timeUnlocked")?.as_str())
        .flatten()
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .and_then(|date| u64::try_from(date.timestamp()).ok());
    let progress = row
        .pointer("/progression/requirements")
        .and_then(Value::as_array)
        .and_then(|requirements| match requirements.as_slice() {
            [requirement] => {
                let current = number(&requirement["current"])?;
                let target = number(&requirement["target"])?;
                (target > 0.).then(|| (current / target).clamp(0., 1.))
            }
            _ => None,
        });
    Some(Achievement {
        id,
        name: row["name"].as_str().unwrap_or("Hidden achievement").into(),
        description: row[description].as_str().unwrap_or_default().into(),
        icon,
        unlocked,
        hidden: row["isSecret"] == true,
        unlocked_at,
        progress,
        rarity: row
            .pointer("/rarity/currentPercentage")
            .and_then(number)
            .filter(|n| (0. ..=100.).contains(n)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paged_achievements_keep_progress_and_hide_locked_secrets() {
        let unlocked = json!({
            "id": "1", "serviceConfigId": "a", "name": "Welcome",
            "description": "Finished the first quest", "progressState": "Achieved",
            "titleAssociations": [{"id": 123}], "isSecret": true,
            "progression": {"timeUnlocked": "2026-11-21T19:12:00Z"},
            "mediaAssets": [{"type": "Icon", "url": "https://images.example/icon.png"}],
            "rarity": {"currentPercentage": 3.5}
        });
        let locked = json!({
            "id": "1", "serviceConfigId": "b", "name": "Explorer",
            "description": "Contains a spoiler", "lockedDescription": "Explore more",
            "progressState": "InProgress", "titleAssociations": [{"id": "123"}],
            "isSecret": true,
            "progression": {"requirements": [{"current": "2", "target": "5"}]}
        });
        let achievements = fetch(123, |token| match token {
            None => Some(json!({
                "achievements": [unlocked, {"titleAssociations": [{"id": 456}]}],
                "pagingInfo": {"continuationToken": "next"}
            })),
            Some("next") => Some(json!({
                "achievements": [unlocked, locked, {"isRevoked": true}],
                "pagingInfo": {"continuationToken": null}
            })),
            _ => None,
        })
        .unwrap();
        let [earned, in_progress] = achievements.as_slice() else {
            panic!("Expected distinct achievements in both service configurations");
        };
        assert!(earned.unlocked && !earned.concealed());
        assert!(earned.unlocked_at.is_some());
        assert_eq!(earned.icon, "https://images.example/icon.png");
        assert_eq!(earned.rarity, Some(3.5));
        assert!(in_progress.concealed());
        assert_eq!(in_progress.completion(), 0.4);
        assert_eq!(in_progress.description, "Explore more");
    }

    #[test]
    fn incomplete_or_invalid_results_cannot_replace_the_cache() {
        let first = || json!({"achievements": [], "pagingInfo": {"continuationToken": "next"}});
        assert!(fetch(123, |token| token.is_none().then(first)).is_none());
        assert!(fetch(123, |_| Some(first())).is_none());
        assert!(fetch(123, |_| Some(json!({"achievements": [{}]}))).is_none());
        assert_eq!(
            fetch(123, |_| Some(json!({"achievements": []})))
                .unwrap()
                .len(),
            0
        );
    }
}

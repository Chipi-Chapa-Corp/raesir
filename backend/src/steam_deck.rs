use crate::storage;
use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, Serialize};
use std::{
    io::Read as _,
    path::Path,
    sync::LazyLock,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Compatibility {
    Unsupported,
    Playable,
    Verified,
}

#[derive(Deserialize, Serialize)]
struct Rating {
    checked: u64,
    compatibility: Option<Compatibility>,
}

pub fn rating(cache: &Path, app_id: u32) -> Option<Compatibility> {
    let path = cache.join("steam-deck").join(format!("{app_id}.json"));
    let previous = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Rating>(&bytes).ok());
    let now = UNIX_EPOCH.elapsed().unwrap_or_default().as_secs();
    if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some()
        || previous
            .as_ref()
            .is_some_and(|rating| now.saturating_sub(rating.checked) < 86400)
    {
        return previous.and_then(|rating| rating.compatibility);
    }
    match fetch(app_id, now) {
        Some(rating) => {
            let _ = storage::write_json(&path, &rating);
            rating.compatibility
        }
        None => previous.and_then(|rating| rating.compatibility),
    }
}

fn fetch(app_id: u32, now: u64) -> Option<Rating> {
    static CLIENT: LazyLock<Option<Client>> = LazyLock::new(|| {
        Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .user_agent("ExposedGames/0.1")
            .build()
            .ok()
    });
    let response = CLIENT
        .as_ref()?
        .get("https://store.steampowered.com/saleaction/ajaxgetdeckappcompatibilityreport")
        .query(&[("nAppID", app_id)])
        .send()
        .ok()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Some(Rating {
            checked: now,
            compatibility: None,
        });
    }
    let summary: serde_json::Value =
        serde_json::from_reader(response.error_for_status().ok()?.take(16 * 1024)).ok()?;
    report_rating(&summary, app_id, now)
}

fn report_rating(report: &serde_json::Value, app_id: u32, checked: u64) -> Option<Rating> {
    let results = &report["results"];
    if report["success"] != 1 || results["appid"] != app_id {
        return None;
    }
    let compatibility = match results["resolved_category"].as_u64() {
        Some(1) => Some(Compatibility::Unsupported),
        Some(2) => Some(Compatibility::Playable),
        Some(3) => Some(Compatibility::Verified),
        _ => None,
    };
    Some(Rating {
        checked,
        compatibility,
    })
}

#[cfg(test)]
#[test]
fn deck_report_distinguishes_unsupported_from_unknown() {
    for (category, expected) in [
        (0, None),
        (1, Some(Compatibility::Unsupported)),
        (2, Some(Compatibility::Playable)),
        (3, Some(Compatibility::Verified)),
        (4, None),
    ] {
        let report = serde_json::json!({"success":1,"results":{
            "appid":42,"resolved_category":category,"steamos_resolved_category":3
        }});
        assert_eq!(
            report_rating(&report, 42, 1).unwrap().compatibility,
            expected
        );
        assert!(report_rating(&report, 43, 1).is_none());
    }
}

use crate::{details, storage};
use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read as _,
    path::Path,
    sync::LazyLock,
    time::{Duration, UNIX_EPOCH},
};

static CLIENT: LazyLock<Option<Client>> = LazyLock::new(|| {
    Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .user_agent("Raesir/0.1")
        .build()
        .ok()
});

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamEdition {
    Missing,
    Listed(Option<Compatibility>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum StoreMatch {
    Missing,
    App(u32),
}

#[derive(Deserialize, Serialize)]
struct EditionMatch {
    checked: u64,
    title: String,
    result: StoreMatch,
}

pub fn xbox_edition(cache: &Path, product_id: &str, title: &str) -> Option<SteamEdition> {
    let path = crate::xbox_store::product_id(product_id).then(|| {
        cache
            .join("xbox-steam-editions")
            .join(format!("{}.json", product_id.to_ascii_uppercase()))
    });
    let previous = path
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<EditionMatch>(&bytes).ok())
        .filter(|previous| previous.title == title);
    let now = UNIX_EPOCH.elapsed().unwrap_or_default().as_secs();
    let mut result = previous.as_ref().map(|previous| previous.result);
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_none()
        && previous
            .as_ref()
            .is_none_or(|previous| now.saturating_sub(previous.checked) >= 86400)
        && let Some(client) = CLIENT.as_ref()
    {
        let query = xbox_base_title(title);
        if let Ok(response) = details::steam_store_search(client, query)
            && let Some(found) = match_store_search(&response, title)
        {
            result = Some(found);
            if let Some(path) = &path {
                let _ = storage::write_json(
                    path,
                    &EditionMatch {
                        checked: now,
                        title: title.into(),
                        result: found,
                    },
                );
            }
        }
    }
    Some(match result? {
        StoreMatch::Missing => SteamEdition::Missing,
        StoreMatch::App(id) => SteamEdition::Listed(rating(cache, id)),
    })
}

fn xbox_base_title(title: &str) -> &str {
    let title = title.trim();
    let lower = title.to_lowercase();
    for suffix in [
        " standard edition",
        " for windows",
        " windows edition",
        " (windows)",
        " (pc)",
    ] {
        if lower.ends_with(suffix)
            && let Some(base) = title.get(..title.len() - suffix.len()).map(str::trim_end)
            && !base.is_empty()
        {
            return base;
        }
    }
    title
}

fn match_store_search(response: &Value, title: &str) -> Option<StoreMatch> {
    let items = response["items"].as_array()?;
    let find = |title| {
        let mut ids = items.iter().filter_map(|row| {
            let id = u32::try_from(row["id"].as_u64()?).ok()?;
            (id != 0 && row["type"] == "app" && details::same_title(row["name"].as_str()?, title))
                .then_some(id)
        });
        let first = ids.next();
        if first.is_some_and(|first| ids.any(|id| id != first)) {
            return None;
        }
        Some(first)
    };
    let id = match find(title)? {
        Some(id) => Some(id),
        None => find(xbox_base_title(title))?,
    };
    if let Some(id) = id {
        return Some(StoreMatch::App(id));
    }
    // Failed and truncated searches cannot establish that a Steam edition is missing.
    (response["total"].as_u64()? <= items.len() as u64).then_some(StoreMatch::Missing)
}

pub fn rating(cache: &Path, app_id: u32) -> Option<Compatibility> {
    let path = cache.join("steam-deck").join(format!("{app_id}.json"));
    let previous = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Rating>(&bytes).ok());
    let now = UNIX_EPOCH.elapsed().unwrap_or_default().as_secs();
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some()
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

fn report_rating(report: &Value, app_id: u32, checked: u64) -> Option<Rating> {
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

#[cfg(test)]
#[test]
fn store_search_requires_an_unambiguous_matching_steam_title() {
    use serde_json::json;
    let base = json!({"id": 42, "type": "app", "name": "Forza Horizon 6"});
    let dlc = json!({"id": 43, "type": "app", "name": "Forza Horizon 6 Premium Upgrade"});
    let search = json!({"total": 2, "items": [base, dlc]});
    assert_eq!(
        match_store_search(&search, "Forza Horizon 6 Standard Edition"),
        Some(StoreMatch::App(42))
    );
    assert_eq!(
        match_store_search(&search, "Forza Horizon 6 Deluxe Edition"),
        Some(StoreMatch::Missing)
    );
    let other = json!({"id": 44, "type": "app", "name": "Forza Horizon 6"});
    let ambiguous = json!({"total": 2, "items": [base, other]});
    assert_eq!(match_store_search(&ambiguous, "Forza Horizon 6"), None);
}

#[cfg(test)]
#[test]
fn failed_or_incomplete_steam_searches_do_not_claim_the_edition_is_missing() {
    use serde_json::json;
    assert_eq!(
        match_store_search(&json!({"total": 0, "items": []}), "Retro Classics"),
        Some(StoreMatch::Missing),
    );
    for response in [json!({}), json!({"total": 10, "items": []})] {
        assert_eq!(match_store_search(&response, "Retro Classics"), None);
    }
}

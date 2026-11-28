use crate::{details, storage};
use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::Read as _,
    path::Path,
    sync::{LazyLock, Mutex},
    time::Duration,
};

static INDEX_LOCK: Mutex<()> = Mutex::new(());

pub mod reviews;

// Ratings are optional; store-search helpers called with this client inherit its timeout.
static CLIENT: crate::http::Shared = LazyLock::new(|| {
    crate::http::build(Client::builder().https_only(true), Duration::from_secs(5))
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Native,
    Platinum,
    Gold,
    Silver,
    Bronze,
    Borked,
}

#[derive(Deserialize, Serialize)]
struct Rating {
    checked: u64,
    tier: Option<Tier>,
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamEdition {
    Missing,
    Listed(Option<Tier>),
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
    let now = crate::util::unix_now();
    let mut result = previous.as_ref().map(|previous| previous.result);
    if !crate::http::network_disabled()
        && previous
            .as_ref()
            .is_none_or(|previous| now.saturating_sub(previous.checked) >= 86400)
        && let Ok(client) = CLIENT.as_ref()
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
                update_xbox_index(cache, title, found);
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

pub fn rating(cache: &Path, app_id: u32) -> Option<Tier> {
    load_rating(cache, app_id, false)?.tier
}

pub fn review_count(cache: &Path, app_id: u32) -> Option<u64> {
    load_rating(cache, app_id, true)?.total
}

fn index_is_current(dir: &Path, index: &Path) -> bool {
    let Ok(index_mtime) = std::fs::metadata(index).and_then(|m| m.modified()) else {
        return false;
    };
    let Ok(dir_mtime) = std::fs::metadata(dir).and_then(|m| m.modified()) else {
        return true;
    };
    index_mtime >= dir_mtime
}

pub fn cached_ratings(cache: &Path) -> HashMap<u32, Tier> {
    let dir = cache.join("protondb");
    let index = dir.join("index.json");
    if index_is_current(&dir, &index)
        && let Ok(bytes) = std::fs::read(&index)
        && let Ok(map) = serde_json::from_slice::<HashMap<u32, Tier>>(&bytes)
    {
        return map;
    }
    let map: HashMap<u32, Tier> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let id = path.file_stem()?.to_str()?.parse().ok()?;
            Some((id, read_rating(&path)?.tier?))
        })
        .collect();
    if !map.is_empty() {
        let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = storage::write_json(&index, &map);
    }
    map
}

pub fn cached_xbox_editions(
    cache: &Path,
    ratings: &HashMap<u32, Tier>,
) -> HashMap<String, SteamEdition> {
    let dir = cache.join("xbox-steam-editions");
    let index = dir.join("index.json");
    let matches = if index_is_current(&dir, &index)
        && let Ok(bytes) = std::fs::read(&index)
        && let Ok(map) = serde_json::from_slice::<HashMap<String, StoreMatch>>(&bytes)
    {
        map
    } else {
        let map: HashMap<String, StoreMatch> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "json" || path.file_name()? == "index.json" {
                    return None;
                }
                let bytes = std::fs::read(&path).ok()?;
                let entry: EditionMatch = serde_json::from_slice(&bytes).ok()?;
                Some((entry.title, entry.result))
            })
            .collect();
        if !map.is_empty() {
            let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let _ = storage::write_json(&index, &map);
        }
        map
    };
    matches
        .into_iter()
        .map(|(title, result)| {
            let edition = match result {
                StoreMatch::Missing => SteamEdition::Missing,
                StoreMatch::App(id) => SteamEdition::Listed(ratings.get(&id).copied()),
            };
            (title, edition)
        })
        .collect()
}

fn update_rating_index(cache: &Path, app_id: u32, tier: Option<Tier>) {
    let index = cache.join("protondb").join("index.json");
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map: HashMap<u32, Tier> = std::fs::read(&index)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let changed = match tier {
        Some(tier) => map.insert(app_id, tier) != Some(tier),
        None => map.remove(&app_id).is_some(),
    };
    if changed {
        let _ = storage::write_json(&index, &map);
    }
}

fn update_xbox_index(cache: &Path, title: &str, result: StoreMatch) {
    let index = cache.join("xbox-steam-editions").join("index.json");
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map: HashMap<String, StoreMatch> = std::fs::read(&index)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if map.insert(title.to_owned(), result) != Some(result) {
        let _ = storage::write_json(&index, &map);
    }
}

fn read_rating(path: &Path) -> Option<Rating> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn load_rating(cache: &Path, app_id: u32, require_count: bool) -> Option<Rating> {
    let path = cache.join("protondb").join(format!("{app_id}.json"));
    let previous = read_rating(&path);
    let now = crate::util::unix_now();
    if crate::http::network_disabled()
        || previous.as_ref().is_some_and(|rating| {
            now.saturating_sub(rating.checked) < 86400 && (!require_count || rating.total.is_some())
        })
    {
        return previous;
    }
    match fetch(app_id, now) {
        Some(rating) => {
            let _ = storage::write_json(&path, &rating);
            update_rating_index(cache, app_id, rating.tier);
            Some(rating)
        }
        None => previous,
    }
}

fn fetch(app_id: u32, now: u64) -> Option<Rating> {
    let response = CLIENT
        .as_ref()
        .ok()?
        .get(format!(
            "https://www.protondb.com/api/v1/reports/summaries/{app_id}.json"
        ))
        .send()
        .ok()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Some(Rating {
            checked: now,
            tier: None,
            total: Some(0),
        });
    }
    let summary: Value =
        serde_json::from_reader(response.error_for_status().ok()?.take(16 * 1024)).ok()?;
    report_rating(&summary, now)
}

fn report_rating(summary: &Value, checked: u64) -> Option<Rating> {
    let tier = match summary.get("tier")?.as_str()? {
        "native" => Some(Tier::Native),
        "platinum" => Some(Tier::Platinum),
        "gold" => Some(Tier::Gold),
        "silver" => Some(Tier::Silver),
        "bronze" => Some(Tier::Bronze),
        "borked" => Some(Tier::Borked),
        _ => None,
    };
    Some(Rating {
        checked,
        tier,
        total: summary["total"].as_u64(),
    })
}

#[cfg(test)]
#[test]
fn protondb_uses_the_overall_tier_and_leaves_unrated_games_unknown() {
    use serde_json::json;
    for (tier, expected) in [
        ("native", Some(Tier::Native)),
        ("platinum", Some(Tier::Platinum)),
        ("gold", Some(Tier::Gold)),
        ("silver", Some(Tier::Silver)),
        ("bronze", Some(Tier::Bronze)),
        ("borked", Some(Tier::Borked)),
        ("pending", None),
    ] {
        let summary = json!({"tier": tier, "trendingTier": "platinum"});
        assert_eq!(report_rating(&summary, 1).unwrap().tier, expected);
    }
    assert!(report_rating(&json!({"trendingTier": "gold"}), 1).is_none());
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

#[cfg(test)]
#[test]
fn consolidated_indexes_load_cached_protondb_and_xbox_editions() {
    let dir = tempfile::tempdir().unwrap();
    storage::write_json(
        &dir.path().join("protondb/42.json"),
        &Rating {
            checked: 100,
            tier: Some(Tier::Platinum),
            total: Some(50),
        },
    )
    .unwrap();
    storage::write_json(
        &dir.path().join("xbox-steam-editions/9MT5NJ5W7B8Z.json"),
        &EditionMatch {
            checked: 100,
            title: "Fixture Game".into(),
            result: StoreMatch::App(42),
        },
    )
    .unwrap();
    let ratings = cached_ratings(dir.path());
    assert_eq!(ratings.get(&42), Some(&Tier::Platinum));
    assert!(dir.path().join("protondb/index.json").is_file());

    let editions = cached_xbox_editions(dir.path(), &ratings);
    assert_eq!(
        editions.get("Fixture Game"),
        Some(&SteamEdition::Listed(Some(Tier::Platinum)))
    );
    assert!(dir.path().join("xbox-steam-editions/index.json").is_file());

    // Subsequent calls read directly from index.json even without individual files.
    std::fs::remove_file(dir.path().join("protondb/42.json")).unwrap();
    std::fs::remove_file(dir.path().join("xbox-steam-editions/9MT5NJ5W7B8Z.json")).unwrap();
    // Touch index.json so its mtime is >= the directory mtime after removing the entries.
    let now = std::time::SystemTime::now() + Duration::from_secs(2);
    for idx in ["protondb/index.json", "xbox-steam-editions/index.json"] {
        std::fs::File::options()
            .write(true)
            .open(dir.path().join(idx))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(now))
            .unwrap();
    }
    let ratings2 = cached_ratings(dir.path());
    assert_eq!(ratings2.get(&42), Some(&Tier::Platinum));
    let editions2 = cached_xbox_editions(dir.path(), &ratings2);
    assert_eq!(
        editions2.get("Fixture Game"),
        Some(&SteamEdition::Listed(Some(Tier::Platinum)))
    );
}

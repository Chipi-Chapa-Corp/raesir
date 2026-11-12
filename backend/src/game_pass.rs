//! Public PC Game Pass catalog. Catalog membership is separate from purchases.
use crate::{catalog, model::*, storage};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) const SOURCE: &str = "game-pass:pc";
// Same PC collection, platform and subscription context as Xbox's catalog page.
const LIST: &str = "https://catalog.gamepass.com/sigls/v3?id=97c6c862-d28a-4907-a3d5-c401f2296a53&language=en-us&market=US&platformContext=pc&subscriptionContext=cfq7ttc0khs0";

#[derive(Serialize, Deserialize)]
struct Catalog {
    updated: u64,
    games: Vec<LibraryGame>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cached(dir: &Path) -> Option<Catalog> {
    serde_json::from_slice(&fs::read(dir.join("game-pass.json")).ok()?).ok()
}

pub fn load(dir: &Path, report: &mut Report) {
    if let Some(catalog) = cached(dir) {
        apply(report, &catalog);
    }
}

/// Refresh off the UI thread; keep the last complete catalog on failure.
pub fn refresh(dir: &Path, report: &mut Report, force: bool) -> Result<bool, String> {
    if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
        return Ok(false);
    }
    if !force && cached(dir).is_some_and(|c| now().saturating_sub(c.updated) < 24 * 60 * 60) {
        return Ok(false);
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("ExposedGames/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let ids = product_ids(&json(&client, LIST)?)?;
    let mut games = Vec::with_capacity(ids.len());
    for batch in ids.chunks(50) {
        let url = format!(
            "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds={}&market=US&languages=en-US",
            batch.join(",")
        );
        let response = json(&client, &url)?;
        let products = response["Products"]
            .as_array()
            .ok_or("Invalid Game Pass product metadata")?;
        games.extend(products.iter().filter_map(product));
    }
    let received: BTreeSet<_> = games.iter().map(|g| g.id.as_str()).collect();
    if ids.iter().any(|id| !received.contains(id.as_str())) {
        return Err("Game Pass returned an incomplete catalog; keeping the previous list".into());
    }
    let catalog = Catalog {
        updated: now(),
        games,
    };
    storage::write_json(&dir.join("game-pass.json"), &catalog)?;
    apply(report, &catalog);
    Ok(true)
}

fn json(client: &Client, url: &str) -> Result<Value, String> {
    let mut bytes = Vec::new();
    client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Game Pass catalog could not be refreshed: {e}"))?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Game Pass catalog response is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid Game Pass catalog response".into())
}

fn store_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn product_ids(value: &Value) -> Result<Vec<String>, String> {
    let rows = value.as_array().ok_or("Invalid Game Pass game list")?;
    let ids: BTreeSet<String> = rows
        .iter()
        .filter_map(|r| r["id"].as_str())
        .filter(|id| store_id(id))
        .map(str::to_owned)
        .collect();
    if ids.is_empty() || ids.len() > 5000 {
        return Err("Game Pass returned an invalid game list".into());
    }
    Ok(ids.into_iter().collect())
}

fn product(value: &Value) -> Option<LibraryGame> {
    let id = value["ProductId"].as_str().filter(|id| store_id(id))?;
    if value["ProductKind"].as_str() != Some("Game") {
        return None;
    }
    let localized = value["LocalizedProperties"].as_array()?.first()?;
    let title = localized["ProductTitle"]
        .as_str()
        .filter(|t| !t.trim().is_empty())?;
    let package = value["Properties"]["PackageIdentityName"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let publisher = localized["PublisherName"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let store = if package.starts_with("ubisoft.") || publisher == "ubisoft" {
        "ubisoft"
    } else if package.starts_with("electronicarts.") || publisher == "electronic arts" {
        "ea"
    } else {
        "xbox"
    };
    let mut game = catalog::entry(store, id, title, vec![], SOURCE);
    game.game_pass = true;
    game.game_pass_only = true;
    game.in_library = true;
    game.is_game = Some(true);
    game.client = "Xbox".into();
    game.client_os = vec![Os::Windows];
    let images = localized["Images"].as_array();
    game.cover = images
        .and_then(|images| {
            images
                .iter()
                .find(|image| image["ImagePurpose"] == "Poster")
                .or_else(|| {
                    images
                        .iter()
                        .find(|image| image["ImagePurpose"] == "BoxArt")
                })
        })
        .and_then(|image| image["Uri"].as_str())
        .and_then(|uri| {
            let uri = if uri.starts_with("//") {
                format!("https:{uri}")
            } else {
                uri.to_owned()
            };
            let mut url = reqwest::Url::parse(&uri).ok()?;
            if url.scheme() != "https" {
                return None;
            }
            // Request card-sized artwork rather than multi-megapixel originals.
            url.query_pairs_mut().append_pair("w", "272");
            Some(url.into())
        });
    game.store_page = Some(catalog::url(
        "Xbox",
        format!("https://www.xbox.com/games/store/_/{id}"),
    ));
    game.library = Some(catalog::url("Xbox", "ms-xbox://".into()));
    Some(game)
}

fn title_key(title: &str) -> String {
    let title = title.to_lowercase().replace('&', " and ");
    let mut key: String = title
        .split(|c: char| !c.is_alphanumeric())
        .map(|word| match word {
            "ii" => "2",
            "iii" => "3",
            "iv" => "4",
            _ => word,
        })
        .collect();
    for suffix in [
        "standardedition",
        "goldedition",
        "deluxeedition",
        "standalone",
        "standard",
    ] {
        if let Some(base) = key.strip_suffix(suffix) {
            key = base.to_owned();
            break;
        }
    }
    key
}

fn apply(report: &mut Report, pass: &Catalog) {
    report.catalog.retain(|game| game.source != SOURCE);
    // Reset catalog-derived flags before applying the current membership list.
    for game in report.catalog.iter_mut().filter(|g| g.store == "xbox") {
        game.game_pass = false;
    }
    for incoming in &pass.games {
        let existing = report.catalog.iter_mut().find(|game| {
            game.store == incoming.store
                && (game.id == incoming.id
                    || (matches!(game.store.as_str(), "ubisoft" | "ea")
                        && title_key(&game.title) == title_key(&incoming.title)))
        });
        if let Some(game) = existing {
            // Preserve the discovered launcher ID, installations and native actions.
            game.game_pass = true;
            if game.store == "xbox" && !game.in_library {
                game.game_pass_only = true;
            }
            game.in_library = true;
            catalog::add_os(&mut game.client_os, Os::Windows);
            if game.title == game.id {
                game.title = incoming.title.clone();
            }
            if game.cover.is_none() {
                game.cover = incoming.cover.clone();
            }
        } else {
            report.catalog.push(incoming.clone());
        }
    }
    report
        .catalog
        .sort_by_cached_key(|g| (g.title.to_lowercase(), g.key.clone()));
    catalog::hydrate_install_actions(report);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({"ProductId":"9NP1LC5MX609","ProductKind":"Game",
        "Properties":{"PackageIdentityName":"Ubisoft.PCCHILDOFLIGHT"},
        "LocalizedProperties":[{"ProductTitle":"Child of Light", "Images":[
            {"ImagePurpose":"Poster","Uri":"//store-images.s-microsoft.com/image/test"}
        ]}]})
    }

    #[test]
    fn catalog_includes_uninstalled_games_with_posters_and_their_launcher() {
        let game = product(&fixture()).unwrap();
        assert_eq!(game.store, "ubisoft");
        assert!(game.game_pass && game.in_library);
        assert!(game.installations.is_empty());
        assert!(game.cover.unwrap().contains("w=272"));
        assert_eq!(game.client_os, vec![Os::Windows]);
        assert!(product(&json!({"ProductId":"not-an-id"})).is_none());
        assert_eq!(
            product_ids(&json!([{"title":"All PC Games"},
            {"id":"9NP1LC5MX609"},{"id":"9NP1LC5MX609"},{"id":"invalid"}]))
            .unwrap(),
            vec!["9NP1LC5MX609"]
        );
        assert_eq!(
            title_key("Assassin's Creed® II"),
            title_key("Assassin's Creed 2")
        );
        assert_eq!(
            title_key("Riders Republic"),
            title_key("Riders Republic Standard Edition")
        );
        assert_eq!(
            title_key("Heroes of Might & Magic III"),
            title_key("Heroes of Might and Magic 3")
        );
        assert_ne!(
            title_key("Ghost Recon Wildlands"),
            title_key("Ghost Recon Breakpoint")
        );
    }

    #[test]
    fn installed_subscription_stays_out_of_purchased_after_repeated_catalog_loads() {
        let mut report: Report = serde_json::from_value(json!({"host_os":"linux",
            "games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null}))
        .unwrap();
        let mut incoming = product(&fixture()).unwrap();
        incoming.store = "xbox".into();
        incoming.key = format!("xbox:{}", incoming.id);
        let mut installed =
            catalog::entry("xbox", &incoming.id, &incoming.title, vec![], "installed");
        installed.installations.push(Installation {
            library: "installed".into(),
            path: "fixture".into(),
            os: Os::Windows,
            state: "installed".into(),
            last_seen: 0,
        });
        report.catalog.push(installed);
        let pass = Catalog {
            updated: 1,
            games: vec![incoming],
        };
        for _ in 0..3 {
            apply(&mut report, &pass);
            assert_eq!(report.catalog.len(), 1);
            assert!(report.catalog[0].game_pass && report.catalog[0].game_pass_only);
            assert_eq!(report.catalog[0].installations.len(), 1);
        }
        let dir = tempfile::tempdir().unwrap();
        storage::write_json(&dir.path().join("game-pass.json"), &pass).unwrap();
        load(dir.path(), &mut report);
        assert!(report.catalog[0].game_pass_only);
        apply(
            &mut report,
            &Catalog {
                updated: 2,
                games: vec![],
            },
        );
        assert!(!report.catalog[0].game_pass);
        assert!(
            report.catalog[0].game_pass_only,
            "losing Game Pass access does not prove a purchase"
        );
        assert_eq!(report.catalog[0].installations.len(), 1);
    }

    #[test]
    fn refresh_merges_local_installs_and_removes_departed_catalog_entries() {
        let mut report: Report = serde_json::from_value(json!({"host_os":"linux",
            "games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null}))
        .unwrap();
        let mut local = catalog::entry("ubisoft", "609", "Child of Light™", vec![], "local");
        local.launch = Some(catalog::url("Ubisoft Connect", "uplay://launch/609".into()));
        report.catalog.push(local);
        let mut xbox = product(&fixture()).unwrap();
        xbox.store = "xbox".into();
        xbox.key = format!("xbox:{}", xbox.id);
        xbox.title = "Other game".into();
        let pass = Catalog {
            updated: 1,
            games: vec![product(&fixture()).unwrap(), xbox],
        };
        apply(&mut report, &pass);
        assert_eq!(report.catalog.len(), 2);
        let game = report.catalog.iter().find(|g| g.id == "609").unwrap();
        assert!(game.game_pass);
        assert!(game.launch.is_some());
        apply(&mut report, &pass);
        assert_eq!(report.catalog.len(), 2);
        apply(
            &mut report,
            &Catalog {
                updated: 2,
                games: vec![],
            },
        );
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].id, "609");
    }
}

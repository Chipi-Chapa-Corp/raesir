use crate::{game_pass, model::Report, storage};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const FILE: &str = "xbox-store.json";
pub(crate) const COMPATIBILITY_VERSION: u8 = 2;

#[derive(Deserialize, Serialize)]
struct Product {
    checked: u64,
    #[serde(default)]
    compatibility_version: u8,
    console_only: bool,
    #[serde(default)]
    ferestre_unsupported: Option<bool>,
    #[serde(default)]
    pc_package: bool,
    title: Option<String>,
}

#[derive(Deserialize)]
struct ProductResponse {
    #[serde(rename = "Products")]
    products: Vec<Value>,
}

pub(crate) fn product_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub(crate) fn achievement_title_id(id: &str) -> Option<u32> {
    let products = products(&[id], "US")?;
    let product = products.iter().find(|p| {
        p["ProductId"]
            .as_str()
            .is_some_and(|product_id| product_id.eq_ignore_ascii_case(id))
    })?;
    xbox_title_id(product)
}

fn xbox_title_id(product: &Value) -> Option<u32> {
    let mut ids = product["AlternateIds"]
        .as_array()?
        .iter()
        .filter(|entry| entry["IdType"] == "XboxTitleId")
        .filter_map(|entry| entry["Value"].as_str()?.parse::<u32>().ok())
        .filter(|&id| id != 0);
    let id = ids.next()?;
    ids.all(|other| other == id).then_some(id)
}

pub(crate) fn products(ids: &[&str], market: &str) -> Option<Vec<Value>> {
    if ids.is_empty()
        || ids.iter().any(|id| !product_id(id))
        || std::env::var_os("RAESIR_DISABLE_NETWORK").is_some()
    {
        return None;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .https_only(true)
        .build()
        .ok()?;
    let response = client
        .get("https://displaycatalog.mp.microsoft.com/v7.0/products")
        .query(&[
            ("bigIds", ids.join(",")),
            ("market", market.into()),
            ("languages", "en-US,en,neutral".into()),
        ])
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .ok()?;
    let value: ProductResponse = serde_json::from_reader(response.take(32 * 1024 * 1024)).ok()?;
    Some(value.products)
}

fn cached(dir: &Path) -> BTreeMap<String, Product> {
    fs::read(dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn load(dir: &Path, report: &mut Report) {
    apply(report, &cached(dir));
}

pub(crate) fn refresh(dir: &Path, report: &mut Report) {
    let mut cache = cached(dir);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ids: BTreeSet<_> = report
        .catalog
        .iter()
        .filter(|game| game.store == "xbox" && game.in_library && game.source != game_pass::SOURCE)
        .map(|game| game.id.as_str())
        .filter(|id| product_id(id))
        .filter(|id| {
            cache.get(&id.to_ascii_uppercase()).is_none_or(|p| {
                p.compatibility_version != COMPATIBILITY_VERSION
                    || now.saturating_sub(p.checked) >= 86400
            })
        })
        .collect();
    let ids: Vec<_> = ids.into_iter().collect();
    let mut changed = false;
    for batch in ids.chunks(25) {
        let Some(rows) = products(batch, "US") else {
            break;
        };
        for id in batch {
            let value = rows.iter().find(|row| {
                row["ProductId"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(id))
            });
            let key = id.to_ascii_uppercase();
            if value.is_none() && cache.contains_key(&key) {
                continue;
            }
            let title = value
                .and_then(|value| value.pointer("/LocalizedProperties/0/ProductTitle"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned);
            use crate::ferestre::PackageSupport;
            let support = value.map(crate::ferestre::package_support);
            cache.insert(
                key,
                Product {
                    checked: now,
                    compatibility_version: COMPATIBILITY_VERSION,
                    console_only: support == Some(PackageSupport::ConsoleOnly),
                    ferestre_unsupported: Some(support == Some(PackageSupport::Unsupported)),
                    pc_package: support == Some(PackageSupport::Msixvc),
                    title,
                },
            );
            changed = true;
        }
    }
    if changed && let Err(error) = storage::write_json(&dir.join(FILE), &cache) {
        report.warnings.push(error);
    }
    apply(report, &cache);
}

fn apply(report: &mut Report, products: &BTreeMap<String, Product>) {
    let linux = report.host_os == crate::model::Os::Linux;
    for game in report
        .catalog
        .iter_mut()
        .filter(|game| game.store == "xbox")
    {
        if let Some(product) = products.get(&game.id.to_ascii_uppercase()) {
            game.xbox_console_only = game.installations.is_empty() && product.console_only;
            game.ferestre_unsupported_package = linux && product.ferestre_unsupported == Some(true);
            game.ferestre_pc_package = product.pc_package;
            if game.title == game.id
                && let Some(title) = &product.title
            {
                game.title.clone_from(title);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, catalog, model::Os};

    #[test]
    fn achievement_title_id_comes_from_unambiguous_xbox_catalog_metadata() {
        let product = |alternate_ids| serde_json::json!({"AlternateIds": alternate_ids});
        let id = |id_type, value| serde_json::json!({"IdType": id_type, "Value": value});
        let other = id("LegacyWindowsStoreProductId", "123");
        let xbox = id("XboxTitleId", "1907796436");
        assert_eq!(
            xbox_title_id(&product(vec![other.clone(), xbox.clone(), xbox.clone()])),
            Some(1907796436)
        );
        assert_eq!(xbox_title_id(&product(vec![other])), None);
        assert_eq!(
            xbox_title_id(&product(vec![xbox, id("XboxTitleId", "456")])),
            None
        );
    }

    #[test]
    fn cached_console_metadata_disables_an_old_install_action() {
        let dir = tempfile::tempdir().unwrap();
        let mut report: Report = serde_json::from_value(serde_json::json!({
            "host_os": Os::host(), "games": [], "libraries": [], "volumes": [],
            "warnings": [], "owned_account": null
        }))
        .unwrap();
        let id = "9NRLQNVKCGWC";
        for (store, product) in [("xbox", id), ("xbox", "9P5786PJB9RP"), ("steam", id)] {
            let mut game = catalog::entry(store, product, product, vec![], "fixture");
            game.in_library = true;
            game.client_os.push(Os::host());
            game.install = Some(catalog::url("Fixture", "https://example.com".into()));
            report.catalog.push(game);
        }
        storage::write_json(&dir.path().join("inventory.json"), &report).unwrap();
        storage::write_json(
            &dir.path().join(FILE),
            &BTreeMap::from([(
                id,
                Product {
                    checked: 1,
                    compatibility_version: COMPATIBILITY_VERSION,
                    console_only: true,
                    ferestre_unsupported: Some(false),
                    pc_package: false,
                    title: Some("Unrailed!".into()),
                },
            )]),
        )
        .unwrap();

        let mut report = storage::cached(dir.path()).unwrap();
        let console = &report.catalog[0];
        assert_eq!(console.title, "Unrailed!");
        assert!(console.xbox_console_only && console.install.is_none());
        let key = console.key.clone();
        assert!(report.catalog[1..].iter().all(actions::can_install));
        report.catalog[0].install = report.catalog[1].install.clone();
        assert!(!actions::can_install(&report.catalog[0]));
        assert_eq!(
            actions::resolve(&report, &key, "install").unwrap_err(),
            crate::ferestre::CONSOLE_ONLY_MESSAGE,
        );
    }
}

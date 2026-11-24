//! Public PC Game Pass catalog. Catalog membership is separate from purchases.
use crate::{catalog, ferestre, model::*, storage};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::Duration,
};

pub(crate) const SOURCE: &str = "game-pass:pc";
const COMPATIBILITY_VERSION: u8 = 4;
// Same PC collection, platform and subscription context as Xbox's catalog page.
const LIST: &str = "https://catalog.gamepass.com/sigls/v3?id=97c6c862-d28a-4907-a3d5-c401f2296a53&language=en-us&market=US&platformContext=pc&subscriptionContext=cfq7ttc0khs0";

#[derive(Default, Serialize, Deserialize)]
struct Catalog {
    updated: u64,
    games: Vec<LibraryGame>,
    #[serde(default)]
    bundle_products: Option<BTreeMap<String, String>>,
    #[serde(default)]
    ferestre_bundles: Option<BTreeSet<String>>,
    #[serde(default)]
    compatibility_version: u8,
}

fn cached(dir: &Path) -> Option<Catalog> {
    serde_json::from_slice(&fs::read(dir.join("game-pass.json")).ok()?).ok()
}

pub fn load(dir: &Path, report: &mut Report) {
    if let Some(mut catalog) = cached(dir) {
        crate::xbox_store::load(dir, report.host_os, &mut catalog.games);
        apply(report, &catalog);
    }
}

/// Refresh off the UI thread; keep the last complete catalog on failure.
pub fn refresh(dir: &Path, report: &mut Report, force: bool) -> Result<bool, String> {
    if crate::http::network_disabled() {
        return Ok(false);
    }
    if !force
        && cached(dir).is_some_and(|c| {
            c.bundle_products.is_some()
                && c.ferestre_bundles.is_some()
                && c.compatibility_version == COMPATIBILITY_VERSION
                && crate::util::unix_now().saturating_sub(c.updated) < 24 * 60 * 60
        })
    {
        return Ok(false);
    }
    let client = crate::http::client()?;
    let ids = product_ids(&json(client, LIST)?)?;
    let mut games = Vec::with_capacity(ids.len());
    let mut bundle_products = BTreeMap::new();
    let mut unmarked_bundles = BTreeMap::new();
    for batch in ids.chunks(50) {
        for value in products(client, batch)? {
            if let Some(game) = product(&value) {
                let members = bundle_members(&value);
                let mut primary = members.iter().filter(|(_, marked)| **marked);
                match (primary.next(), primary.next()) {
                    (Some((id, _)), None) => {
                        bundle_products.insert(game.id.clone(), (*id).to_owned());
                    }
                    (None, None) if game.store == "xbox" && !members.is_empty() => {
                        unmarked_bundles.insert(
                            game.id.clone(),
                            members.into_keys().map(str::to_owned).collect::<Vec<_>>(),
                        );
                    }
                    _ => {}
                }
                games.push(game);
            }
        }
    }
    let received: BTreeSet<_> = games.iter().map(|g| g.id.as_str()).collect();
    if ids.iter().any(|id| !received.contains(id.as_str())) {
        return Err("Game Pass returned an incomplete catalog; keeping the previous list".into());
    }
    let primary_ids: Vec<_> = games
        .iter()
        .filter(|game| game.store == "xbox")
        .filter_map(|game| bundle_products.get(&game.id))
        .chain(unmarked_bundles.values().flatten())
        .filter(|id| !received.contains(id.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut primary_games = Vec::new();
    for batch in primary_ids.chunks(50) {
        primary_games.extend(products(client, batch)?.iter().filter_map(product));
    }
    let products_by_id: BTreeMap<_, _> = games
        .iter()
        .chain(&primary_games)
        .map(|game| (game.id.as_str(), game))
        .collect();
    for (bundle, members) in unmarked_bundles {
        if let Some(id) = matching_bundle_game(
            products_by_id[bundle.as_str()],
            members
                .iter()
                .filter_map(|id| products_by_id.get(id.as_str()).copied()),
        ) {
            bundle_products.insert(bundle, id.to_owned());
        }
    }
    let ferestre_bundles =
        ferestre_bundles(&games, &bundle_products, products_by_id.values().copied());
    let mut catalog = Catalog {
        updated: crate::util::unix_now(),
        games,
        bundle_products: Some(bundle_products),
        ferestre_bundles: Some(ferestre_bundles),
        compatibility_version: COMPATIBILITY_VERSION,
    };
    storage::write_json(&dir.join("game-pass.json"), &catalog)?;
    crate::xbox_store::load(dir, report.host_os, &mut catalog.games);
    apply(report, &catalog);
    Ok(true)
}

fn products(client: &Client, ids: &[String]) -> Result<Vec<Value>, String> {
    let url = format!(
        "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds={}&market=US&languages=en-US",
        ids.join(",")
    );
    let mut response = json(client, &url)?;
    serde_json::from_value(response["Products"].take())
        .map_err(|_| "Invalid Game Pass product metadata".into())
}

fn json(client: &Client, url: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Game Pass catalog could not be refreshed: {e}"))?;
    let bytes = crate::util::read_limited(response, 32 * 1024 * 1024)
        .map_err(|e| e.to_string())?
        .ok_or("Game Pass catalog response is too large")?;
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
    use crate::ferestre::PackageSupport;
    let support = crate::ferestre::package_support(value);
    game.xbox_console_only = support == PackageSupport::ConsoleOnly;
    if game.store == "xbox" {
        game.ferestre_unsupported_package =
            cfg!(target_os = "linux") && support == PackageSupport::Unsupported;
        game.ferestre_pc_package = support == PackageSupport::Msixvc;
    }
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

fn bundle_members(value: &Value) -> BTreeMap<&str, bool> {
    let mut members = BTreeMap::new();
    let rows = value["DisplaySkuAvailabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| &row["Sku"]["Properties"])
        .filter(|properties| properties["IsBundle"] == true)
        .filter_map(|properties| properties["BundledSkus"].as_array())
        .flatten();
    for row in rows {
        if let Some(id) = row["BigId"].as_str().filter(|id| store_id(id)) {
            let primary = members.entry(id).or_insert(false);
            *primary |= row["IsPrimary"] == true;
        }
    }
    members
}

fn bundle_title_key(title: &str) -> String {
    let title = title.to_lowercase();
    let title = title.strip_suffix(" + launcher").unwrap_or(&title);
    let title = title
        .strip_suffix(" for windows")
        .or_else(|| title.strip_suffix(" - windows"))
        .unwrap_or(title);
    title_key(title)
}

fn matching_bundle_game<'a>(
    bundle: &LibraryGame,
    members: impl Iterator<Item = &'a LibraryGame>,
) -> Option<&'a str> {
    let title = bundle_title_key(&bundle.title);
    let mut matching = members.filter(|game| {
        game.store == "xbox" && game.id != bundle.id && bundle_title_key(&game.title) == title
    });
    let game = matching.next()?;
    matching.next().is_none().then_some(game.id.as_str())
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

fn ferestre_bundles<'a>(
    games: &[LibraryGame],
    bundle_products: &BTreeMap<String, String>,
    primary_games: impl Iterator<Item = &'a LibraryGame>,
) -> BTreeSet<String> {
    let pc_games: BTreeMap<_, _> = primary_games
        .filter(|game| game.store == "xbox" && game.ferestre_pc_package)
        .map(|game| (game.id.as_str(), bundle_title_key(&game.title)))
        .collect();
    games
        .iter()
        .filter(|game| {
            game.store == "xbox"
                && bundle_products
                    .get(&game.id)
                    .and_then(|id| pc_games.get(id.as_str()))
                    .is_some_and(|title| *title == bundle_title_key(&game.title))
        })
        .map(|game| game.id.clone())
        .collect()
}

fn local_ferestre(host: Os) -> Option<ferestre::Client> {
    if host != Os::Linux || !ferestre::library_cached(&crate::platform::home()?) {
        return None;
    }
    ferestre::Client::discover()
}

fn apply(report: &mut Report, pass: &Catalog) {
    let client = local_ferestre(report.host_os);
    apply_with_install_actions(report, pass, |id| {
        Some(client.as_ref()?.action(vec!["install".into(), id.into()]))
    });
}

fn apply_with_install_actions(
    report: &mut Report,
    pass: &Catalog,
    ferestre_install: impl Fn(&str) -> Option<GameAction>,
) {
    report.catalog.retain(|game| game.source != SOURCE);
    // Reset catalog-derived flags before applying the current membership list.
    for game in report.catalog.iter_mut().filter(|g| g.store == "xbox") {
        if game.game_pass_only
            && game
                .install
                .as_ref()
                .is_some_and(|a| a.client == "Ferestre")
        {
            game.install = None;
            if game.launch.is_none() {
                game.client = "Xbox".into();
                if !game.installations.iter().any(|i| i.os == Os::Linux) {
                    game.client_os.retain(|os| *os != Os::Linux);
                }
            }
        }
        game.game_pass = false;
    }
    let bundle_product = |game: &LibraryGame| pass.bundle_products.as_ref()?.get(&game.id);
    // Visit base products first so catalog entries can share the same card
    // even when the base game is not installed.
    for incoming in pass
        .games
        .iter()
        .filter(|game| bundle_product(game).is_none())
        .chain(
            pass.games
                .iter()
                .filter(|game| bundle_product(game).is_some()),
        )
    {
        if incoming.xbox_console_only {
            // Reconcile Xbox editions by product ID. Titles shared with a PC
            // edition or a different launcher do not identify the same package.
            if let Some(game) = report
                .catalog
                .iter_mut()
                .find(|game| game.store == "xbox" && game.id.eq_ignore_ascii_case(&incoming.id))
                && game.installations.is_empty()
                && !game.ferestre_pc_package
                && !game.ferestre_unsupported_package
            {
                game.xbox_console_only = true;
            }
            continue;
        }
        let primary = bundle_product(incoming);
        let ferestre_primary = primary.filter(|_| {
            report.host_os == Os::Linux
                && pass
                    .ferestre_bundles
                    .as_ref()
                    .is_some_and(|bundles| bundles.contains(&incoming.id))
        });
        let existing = report.catalog.iter_mut().find(|game| {
            game.store == incoming.store
                && (game.id == incoming.id
                    || (game.store == "xbox"
                        && primary.is_some_and(|id| id == &game.id)
                        && (ferestre_primary.is_some()
                            || game.title == game.id
                            || game.title.eq_ignore_ascii_case(&incoming.title)))
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
            if game.store == "xbox" && game.id == incoming.id {
                // A dated account catalog already provides package evidence.
                // Fill gaps for titles that only had a Windows install record.
                if !game.xbox_console_only
                    && !game.ferestre_unsupported_package
                    && !game.ferestre_pc_package
                {
                    game.ferestre_pc_package = incoming.ferestre_pc_package;
                    game.ferestre_unsupported_package = incoming.ferestre_unsupported_package;
                }
            }
            if ferestre_primary == Some(&game.id)
                && !game.xbox_console_only
                && !game.ferestre_unsupported_package
            {
                game.ferestre_pc_package = true;
            }
        } else {
            let mut game = incoming.clone();
            if let Some(id) = ferestre_primary {
                game.id.clone_from(id);
                game.key = format!("xbox:{id}");
                game.ferestre_pc_package = true;
            }
            report.catalog.push(game);
        }
    }
    if report.host_os == Os::Linux {
        for game in report.catalog.iter_mut().filter(|game| {
            game.store == "xbox"
                && game.game_pass
                && game.game_pass_only
                && game.ferestre_pc_package
                && !game.xbox_console_only
                && !game.ferestre_unsupported_package
                && !catalog::local(&game.installations)
        }) {
            if let Some(action) = ferestre_install(&game.id) {
                game.client = action.client.clone();
                catalog::add_os(&mut game.client_os, Os::Linux);
                game.install = Some(action);
            }
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
    fn console_products_do_not_hide_desktop_editions_with_the_same_title() {
        let mut report: Report = serde_json::from_value(json!({"host_os":"linux",
            "games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null}))
        .unwrap();
        let mut console = fixture();
        console["Properties"] = json!({});
        console["DisplaySkuAvailabilities"] = json!([{"Sku": {"Properties": {"Packages": [{
            "PackageFormat": "MSIXVC", "PlatformDependencies": [{"PlatformName": "Windows.Xbox"}]
        }]}}}]);
        let console_game = product(&console).unwrap();
        let mut mixed = console.clone();
        mixed["ProductId"] = json!("9DESKTOP0001");
        mixed["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]
            ["PlatformDependencies"].as_array_mut().unwrap()
            .push(json!({"PlatformName": "Windows.Desktop"}));
        let mixed_game = product(&mixed).unwrap();
        let mut partner_console = console;
        partner_console["ProductId"] = json!("9CONSOLE0001");
        partner_console["Properties"] = fixture()["Properties"].clone();
        let partner_console_game = product(&partner_console).unwrap();
        assert!(console_game.xbox_console_only && partner_console_game.xbox_console_only);
        assert!(mixed_game.ferestre_pc_package);
        report.catalog.push(catalog::entry(
            "xbox",
            &console_game.id,
            &console_game.title,
            vec![],
            "account",
        ));
        report.catalog.push(catalog::entry(
            "ubisoft",
            "609",
            &console_game.title,
            vec![],
            "pc",
        ));
        let pass = Catalog {
            games: vec![console_game, mixed_game, partner_console_game],
            ..Catalog::default()
        };
        for _ in 0..2 {
            apply_with_install_actions(&mut report, &pass, |_| None);
            let visible: Vec<_> = report
                .catalog
                .iter()
                .filter(|game| catalog::visible(game))
                .collect();
            assert_eq!(visible.len(), 2);
            assert!(
                visible
                    .iter()
                    .any(|game| game.id == "9DESKTOP0001" && game.game_pass)
            );
            assert!(
                visible
                    .iter()
                    .any(|game| game.id == "609" && !game.game_pass)
            );
        }
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
            ..Catalog::default()
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
                ..Catalog::default()
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
    fn xbox_catalog_bundle_uses_the_installed_primary_product_card() {
        let mut report: Report = serde_json::from_value(json!({"host_os":"linux",
            "games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null}))
        .unwrap();
        let mut bundle = fixture();
        bundle["Properties"]["PackageIdentityName"] = json!("");
        bundle["DisplaySkuAvailabilities"] = json!([{"Sku": {"Properties": {
            "IsBundle": true, "BundledSkus": [
                {"BigId": "9P5786PJB9RP", "IsPrimary": true},
                {"BigId": "9PGW18NPBZV5", "IsPrimary": false}
            ]
        }}}]);
        let incoming = product(&bundle).unwrap();
        let primary = bundle_members(&bundle)
            .into_iter()
            .find(|(_, marked)| *marked)
            .unwrap()
            .0;
        let mut installed = catalog::entry("xbox", primary, &incoming.title, vec![], "installed");
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
            bundle_products: Some(BTreeMap::from([(incoming.id.clone(), primary.to_owned())])),
            games: vec![incoming],
            ..Catalog::default()
        };
        apply(&mut report, &pass);
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].id, primary);
        assert_eq!(report.catalog[0].installations.len(), 1);
        assert!(report.catalog[0].game_pass_only);
    }

    #[test]
    fn unmarked_bundle_resolves_the_game_with_the_matching_title() {
        let value = json!({
            "ProductId": "9NM395CH685J",
            "ProductKind": "Game",
            "LocalizedProperties": [{"ProductTitle": "Minecraft Legends for Windows + Launcher"}],
            "DisplaySkuAvailabilities": [{"Sku": {"Properties": {
                "IsBundle": true,
                "BundledSkus": [
                    {"BigId": "9PGW18NPBZV5", "IsPrimary": false},
                    {"BigId": "9N98Z825TNFW", "IsPrimary": false}
                ]
            }}}]
        });
        let bundle = product(&value).unwrap();
        let mut game = catalog::entry(
            "xbox",
            "9N98Z825TNFW",
            "Minecraft Legends - Windows",
            vec![],
            "fixture",
        );
        game.ferestre_pc_package = true;
        let launcher = catalog::entry(
            "xbox",
            "9PGW18NPBZV5",
            "Minecraft Launcher",
            vec![],
            "fixture",
        );
        let members = bundle_members(&value);
        assert_eq!(
            members,
            BTreeMap::from([(game.id.as_str(), false), (launcher.id.as_str(), false)])
        );
        assert_eq!(
            matching_bundle_game(&bundle, [&launcher, &game].into_iter()),
            Some(game.id.as_str())
        );
        assert_eq!(matching_bundle_game(&bundle, [&launcher].into_iter()), None);
        let alternate = catalog::entry(
            "xbox",
            "9ALTGAME0001",
            "Minecraft Legends",
            vec![],
            "fixture",
        );
        assert_eq!(
            matching_bundle_game(&bundle, [&game, &alternate].into_iter()),
            None
        );
        let primary = BTreeMap::from([(bundle.id.clone(), game.id.clone())]);
        let expected = BTreeSet::from([bundle.id.clone()]);
        assert_eq!(
            ferestre_bundles(&[bundle], &primary, [&game, &launcher].into_iter()),
            expected
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn compatible_game_pass_bundle_installs_the_primary_product_through_ferestre() {
        let mut report: Report = serde_json::from_value(json!({"host_os":"linux",
            "games":[],"libraries":[],"volumes":[],"warnings":[],"owned_account":null}))
        .unwrap();
        let bundle = "9PM3J18QM4Q3";
        let primary = "9P5786PJB9RP";
        let mut game_pass = catalog::entry("xbox", bundle, "Minecraft Dungeons II", vec![], SOURCE);
        game_pass.client = "Xbox".into();
        game_pass.client_os = vec![Os::Windows];
        game_pass.in_library = true;
        game_pass.game_pass = true;
        game_pass.game_pass_only = true;

        let mut base = catalog::entry("xbox", primary, &game_pass.title, vec![], "fixture");
        base.ferestre_pc_package = true;
        let mut windows_install = base.clone();
        windows_install.client = "Xbox".into();
        windows_install.client_os.push(Os::Windows);
        windows_install.installations.push(Installation {
            library: "fixture".into(),
            path: "fixture".into(),
            os: Os::Windows,
            state: "unavailable".into(),
            last_seen: 0,
        });
        report.catalog.push(windows_install);

        let action = |id: &str| {
            Some(catalog::command(
                "Ferestre",
                "ferestre-fixture".into(),
                vec!["install".into(), id.into()],
            ))
        };
        let mut pass = Catalog {
            games: vec![game_pass],
            bundle_products: Some(BTreeMap::from([(bundle.into(), primary.into())])),
            ..Catalog::default()
        };
        // A previous catalog can reuse package evidence already discovered for the base game.
        apply_with_install_actions(&mut report, &pass, action);
        assert_eq!(report.catalog[0].id, primary);
        assert!(crate::actions::can_install(&report.catalog[0]));
        assert_eq!(report.catalog[0].installations.len(), 1);

        apply_with_install_actions(&mut report, &pass, |_| None);
        assert_eq!(report.catalog[0].client_os, vec![Os::Windows]);
        assert!(!crate::actions::can_install(&report.catalog[0]));

        // Without a local record, Microsoft package metadata resolves the same edition.
        pass.ferestre_bundles = Some(ferestre_bundles(
            &pass.games,
            pass.bundle_products.as_ref().unwrap(),
            std::iter::once(&base),
        ));
        report.catalog.clear();
        for _ in 0..2 {
            apply_with_install_actions(&mut report, &pass, action);
            assert_eq!(report.catalog.len(), 1);
            let game = &report.catalog[0];
            assert!(game.game_pass && game.game_pass_only);
            assert!(game.client_os.contains(&Os::Linux));
            assert!(
                matches!(crate::actions::resolve(&report, &game.key, "install").unwrap().target,
                    ActionTarget::Command { args, .. } if args == ["install", primary])
            );
        }
        apply_with_install_actions(&mut report, &Catalog::default(), action);
        assert!(report.catalog.is_empty());
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
            ..Catalog::default()
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
                ..Catalog::default()
            },
        );
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].id, "609");
    }
}

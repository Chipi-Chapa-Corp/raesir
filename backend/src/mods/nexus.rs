use super::*;
use serde_json::{Value, json};

const WEBSITE: &str = "https://www.nexusmods.com";
const GAME_CATALOG: &str = "https://data.nexusmods.com/file/nexus-data/games.json";
pub(super) const GRAPHQL: &str = "https://api.nexusmods.com/v2/graphql";
const PAGE_SIZE: u32 = 36;

pub fn has_managed_nexus(game: &LibraryGame) -> bool {
    recipes::for_game(game).is_some()
}

#[derive(Deserialize)]
struct NexusGame {
    name: String,
    domain_name: String,
}

fn game_domain(catalog: &[NexusGame], title: &str) -> Option<String> {
    let title = providers::normalized_title(title);
    if title.is_empty() {
        return None;
    }
    let mut matches = catalog.iter().filter(|entry| {
        !entry.domain_name.is_empty()
            && entry
                .domain_name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
            && providers::normalized_title(&entry.name) == title
    });
    let domain = &matches.next()?.domain_name;
    matches
        .all(|entry| entry.domain_name == *domain)
        .then(|| domain.clone())
}

pub(super) fn browse_url(domain: &str) -> String {
    format!("{WEBSITE}/{domain}/mods/")
}

fn mod_id(url: &str, domain: &str) -> Result<u64, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| "Enter a Nexus mod page URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("www.nexusmods.com" | "nexusmods.com")
        )
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Enter a Nexus Mods HTTPS page URL".into());
    }
    let parts: Vec<_> = parsed
        .path()
        .trim_end_matches('/')
        .split('/')
        .skip(1)
        .collect();
    let parts = parts.strip_prefix(&["games"]).unwrap_or(&parts);
    match parts {
        [game, "mods", id] if *game == domain && id.bytes().all(|b| b.is_ascii_digit()) => {
            if let Ok(id) = id.parse::<u64>()
                && id > 0
            {
                return Ok(id);
            }
        }
        _ => {}
    }
    Err(format!("Choose a {domain} mod page on Nexus"))
}

pub(super) fn mod_url(domain: &str, id: u64) -> String {
    format!("{}{id}", browse_url(domain))
}

pub(super) fn trending_url(domain: &str) -> String {
    format!("https://api.nexusmods.com/v3/games/{domain}/trending-mods")
}

impl Manager {
    pub fn nexus_snapshot(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> Result<Option<Snapshot>, String> {
        if !can_discover_mods(game) {
            return Err("Nexus browsing is available for PC games".into());
        }
        let recipe = recipes::for_game(game);
        let domain = if let Some(recipe) = recipe {
            recipe.nexus_domain.clone()
        } else if let Some(domain) = modde::registration(game).and_then(|rule| rule.nexus_domain) {
            domain.into()
        } else {
            // Vortex's catalog has names and Nexus domains, without store IDs.
            let value = self
                .provider_json_inner(GAME_CATALOG, refresh, true, || {
                    providers::read_json(|| {
                        Ok(providers::http()?
                            .get(GAME_CATALOG)
                            .timeout(Duration::from_secs(10)))
                    })
                })
                .map_err(|error| format!("Could not load the Nexus game catalog: {error}"))?;
            let catalog: Vec<NexusGame> = serde_json::from_value(value)
                .map_err(|error| format!("Invalid Nexus game catalog: {error}"))?;
            let Some(domain) = game_domain(&catalog, &game.title) else {
                return Ok(None);
            };
            domain
        };
        let (profiles, selected) = match recipe {
            Some(recipe) => {
                self.registry(&recipe.id)?;
                let profiles = self.profiles(&recipe.id);
                let selected = self.selected_id(&recipe.id);
                (profiles, selected)
            }
            None => match modde::Target::resolve(game) {
                Some(target) => {
                    let profile = target.profile(self)?;
                    let selected = profile.id.clone();
                    (vec![profile], selected)
                }
                None => (Vec::new(), String::new()),
            },
        };
        Ok(Some(Snapshot {
            nexus_url: Some(browse_url(&domain)),
            community: domain,
            selected,
            profiles,
            packages: Arc::default(),
            warnings: Vec::new(),
        }))
    }

    pub fn nexus_page(
        &self,
        domain: &str,
        query: &str,
        offset: u32,
        order: CatalogOrder,
        refresh: bool,
    ) -> Result<CatalogPage, String> {
        if order == CatalogOrder::Trending {
            let query = query.to_lowercase();
            let mut packages = self.nexus_trending(domain, refresh)?;
            packages.retain(|p| {
                query.is_empty()
                    || format!("{} {}", p.name, p.description)
                        .to_lowercase()
                        .contains(&query)
            });
            return Ok(CatalogPage {
                packages,
                ..Default::default()
            });
        }
        let body = catalog_request(domain, query, offset, order);
        let request = || {
            let response =
                providers::read_json(|| Ok(graphql(&body)?.timeout(Duration::from_secs(10))))?;
            if !response["data"]["mods"]["nodes"].is_array() {
                return Err("Nexus Mods could not return this page of results".into());
            }
            Ok(response)
        };
        let response = if query.is_empty() && offset == 0 {
            self.provider_json_inner(&format!("{GRAPHQL}\n{body}"), refresh, true, request)?
        } else {
            request()?
        };
        decode_page(response, domain, offset)
    }

    pub fn import_nexus(
        &self,
        game: &LibraryGame,
        url: &str,
        archive: &Path,
        folder: &str,
    ) -> Result<Snapshot, String> {
        install::changes_allowed()?;
        if !archive.is_file() {
            return Err("Choose the archive downloaded from Nexus".into());
        }
        if let Some(recipe) = recipes::for_game(game) {
            let id = mod_id(url, &recipe.nexus_domain)?;
            return self.import_noita_nexus(game, id, url, archive, folder);
        }
        let target = modde::Target::resolve(game).ok_or("No supported local game installation")?;
        let id = mod_id(url, target.domain())?;
        let fallback = archive.file_stem().and_then(|name| name.to_str());
        self.import_modde_nexus(game, &target, id, archive, fallback, || false)
    }

    pub(super) fn import_modde_nexus(
        &self,
        game: &LibraryGame,
        target: &modde::Target,
        id: u64,
        archive: &Path,
        fallback: Option<&str>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Snapshot, String> {
        let name = fallback
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Nexus mod {id}"));
        target.install(self, id, archive, name, cancelled)?;
        self.nexus_snapshot(game, false)?
            .ok_or_else(|| "No Nexus listing for this installation".into())
    }

    pub fn uninstall_nexus(&self, game: &LibraryGame, package: &str) -> Result<Snapshot, String> {
        if has_managed_nexus(game) {
            return self.uninstall(game, package);
        }
        install::changes_allowed()?;
        let id = package_id(package)?;
        let target = modde::Target::resolve(game).ok_or("No supported local game installation")?;
        target.uninstall(self, id)?;
        self.nexus_snapshot(game, false)?
            .ok_or_else(|| "No Nexus listing for this installation".into())
    }

    pub(super) fn nexus_trending(
        &self,
        domain: &str,
        refresh: bool,
    ) -> Result<Vec<Package>, String> {
        #[derive(Deserialize)]
        struct TrendingMod {
            name: String,
            author: String,
            summary: String,
            picture_url: Option<String>,
            mod_page_url: String,
        }
        #[derive(Deserialize)]
        struct TrendingResponse {
            data: TrendingData,
        }
        #[derive(Deserialize)]
        struct TrendingData {
            mods: Vec<TrendingMod>,
        }

        let url = trending_url(domain);
        let value = self.provider_json_inner(&url, refresh, true, || {
            providers::read_json(|| Ok(nexus_headers(providers::http()?.get(&url))))
        })?;
        let data: TrendingResponse = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut packages = Vec::new();
        for item in data.data.mods {
            let Ok(id) = mod_id(&item.mod_page_url, domain) else {
                continue;
            };
            let package = package(
                domain,
                id,
                item.name,
                item.author,
                item.summary,
                String::new(),
            );
            if packages.iter().any(|p: &Package| p.id == package.id) {
                continue;
            }
            packages.push(Package {
                icon: image_url(item.picture_url).unwrap_or_default(),
                ..package
            });
        }
        Ok(packages)
    }
}

pub(super) fn graphql(body: &Value) -> Result<reqwest::blocking::RequestBuilder, String> {
    Ok(nexus_headers(providers::http()?.post(GRAPHQL)).json(body))
}

pub(super) fn nexus_headers(
    request: reqwest::blocking::RequestBuilder,
) -> reqwest::blocking::RequestBuilder {
    request
        .header("Application-Name", "Raesir")
        .header("Application-Version", env!("CARGO_PKG_VERSION"))
}

pub(super) fn image_url(url: Option<String>) -> Option<String> {
    url.filter(|url| {
        reqwest::Url::parse(url).is_ok_and(|u| {
            u.scheme() == "https"
                && matches!(
                    u.host_str(),
                    Some("staticdelivery.nexusmods.com" | "images.nexusmods.com")
                )
        })
    })
}

fn catalog_request(domain: &str, query: &str, offset: u32, order: CatalogOrder) -> Value {
    let mut filter = json!({"gameDomainName": [{"op": "EQUALS", "value": domain}]});
    if !query.is_empty() {
        let term = query.chars().take(150).collect::<String>();
        filter["nameStemmed"] = json!([{"op": "MATCHES", "value": term}]);
    }
    let sort = if order == CatalogOrder::Downloads {
        json!([{"downloads": {"direction": "DESC"}}])
    } else if query.is_empty() {
        json!([{"endorsements": {"direction": "DESC"}}])
    } else {
        json!([{"relevance": {"direction": "DESC"}}, {"endorsements": {"direction": "DESC"}}])
    };
    json!({
        "query": "query Catalog($filter: ModsFilter!, $sort: [ModsSort!], $offset: Int!, $count: Int!) { mods(count: $count, offset: $offset, filter: $filter, sort: $sort) { totalCount nodes { modId name author summary version thumbnailLargeUrl thumbnailUrl pictureUrl downloads } } }",
        "variables": {"filter": filter, "sort": sort, "offset": offset, "count": PAGE_SIZE}
    })
}

fn decode_page(response: Value, domain: &str, offset: u32) -> Result<CatalogPage, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Catalog {
        total_count: u32,
        nodes: Vec<Node>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Node {
        mod_id: u64,
        name: String,
        author: Option<String>,
        summary: Option<String>,
        version: Option<String>,
        thumbnail_large_url: Option<String>,
        thumbnail_url: Option<String>,
        picture_url: Option<String>,
        downloads: u64,
    }
    let catalog: Catalog = serde_json::from_value(response["data"]["mods"].clone())
        .map_err(|_| "Nexus Mods could not return this page of results")?;
    let next = offset.saturating_add(catalog.nodes.len() as u32);
    let next = (next > offset && next < catalog.total_count).then_some(next);
    let packages = catalog
        .nodes
        .into_iter()
        .map(|item| Package {
            icon: image_url(item.thumbnail_large_url)
                .or_else(|| image_url(item.thumbnail_url))
                .or_else(|| image_url(item.picture_url))
                .unwrap_or_default(),
            downloads: item.downloads,
            ..package(
                domain,
                item.mod_id,
                item.name,
                item.author.unwrap_or_default(),
                item.summary.unwrap_or_default(),
                item.version.unwrap_or_default(),
            )
        })
        .collect();
    Ok(CatalogPage {
        packages,
        next,
        ..Default::default()
    })
}

pub(super) fn package_id(id: &str) -> Result<u64, String> {
    id.strip_prefix("nexus:")
        .and_then(|id| id.parse().ok())
        .filter(|&id| id > 0)
        .ok_or_else(|| "Invalid Nexus mod identifier".into())
}

pub(super) fn package(
    domain: &str,
    id: u64,
    name: String,
    owner: String,
    description: String,
    version: String,
) -> Package {
    Package {
        id: format!("nexus:{id}"),
        name,
        owner,
        description,
        version,
        page: mod_url(domain, id),
        icon: String::new(),
        downloads: 0,
        deprecated: false,
        categories: Vec::new(),
        source: "Nexus Mods".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(manager: &Manager, url: &str, value: serde_json::Value) {
        let key = launch::hash(url.as_bytes());
        let path = manager.cache.join("providers").join(format!("{key}.json"));
        crate::storage::write_json(&path, &value).unwrap();
    }

    #[test]
    fn browsing_uses_the_catalog_across_stores_without_enabling_installation() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::new(&root.path().join("data"), &root.path().join("cache"));
        cache(
            &manager,
            GAME_CATALOG,
            serde_json::json!([
                {"name":"Fixture Game", "domain_name":"fixturegame"},
                {"name":"Ambiguous Edition", "domain_name":"edition-one"},
                {"name":"Ambiguous Edition", "domain_name":"edition-two"}
            ]),
        );
        for store in ["steam", "epic", "xbox"] {
            let game =
                crate::catalog::entry(store, "unmapped", "FIXTURE™  GAME", vec![], "fixture");
            let snapshot = manager.nexus_snapshot(&game, false).unwrap().unwrap();
            assert_eq!(
                snapshot.nexus_url.as_deref(),
                Some("https://www.nexusmods.com/fixturegame/mods/")
            );
            assert!(snapshot.profiles.is_empty());
            assert!(!has_managed_nexus(&game));
        }
        let mut game =
            crate::catalog::entry("xbox", "unmapped", "Ambiguous Edition", vec![], "fixture");
        assert!(manager.nexus_snapshot(&game, false).unwrap().is_none());
        game.title = "Fixture Game: Definitive Edition".into();
        assert!(manager.nexus_snapshot(&game, false).unwrap().is_none());
        game.title = "Fixture Game".into();
        game.xbox_console_only = true;
        assert!(!can_discover_mods(&game));
    }

    #[test]
    fn search_is_scoped_to_the_game_and_paginates_using_the_returned_count() {
        let request = catalog_request("fixturegame", "rare mod", 36, CatalogOrder::All);
        let filter = &request["variables"]["filter"];
        assert_eq!(filter["gameDomainName"][0]["value"], "fixturegame");
        assert_eq!(filter["nameStemmed"][0]["value"], "rare mod");
        assert_eq!(request["variables"]["offset"], 36);

        let page = decode_page(json!({"data":{"mods":{"totalCount":38,"nodes":[{
            "modId":42,"name":"Fixture mod","downloads":15,"thumbnailLargeUrl":"https://images.nexusmods.com/art.png"
        }]}}}), "fixturegame", 36).unwrap();
        assert_eq!(page.next, Some(37));
        assert_eq!(page.packages[0].id, "nexus:42");
        assert_eq!(
            page.packages[0].icon,
            "https://images.nexusmods.com/art.png"
        );
        assert!(
            decode_page(
                json!({"data":{"mods":{"totalCount":38,"nodes":[]}}}),
                "fixturegame",
                37
            )
            .unwrap()
            .next
            .is_none()
        );
    }
}

use super::*;
use serde_json::Value;

const API: &str = "https://api.modworkshop.net";

impl Manager {
    pub fn modworkshop_game(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> Result<Option<u64>, String> {
        // Recipe games already include ModWorkshop in their managed catalog.
        if recipes::for_game(game).is_some() {
            return Ok(None);
        }
        let title = providers::normalized_title(&game.title);
        if title.is_empty() {
            return Ok(None);
        }
        let query = crate::catalog::encode(&game.title);
        let url = format!("{API}/games?query={query}");
        let data = self.provider_json_ttl(
            &url,
            providers::CATALOG_TTL,
            refresh,
            true,
            || {
                providers::read_json(|| {
                    Ok(providers::http()?.get(&url).timeout(Duration::from_secs(10)))
                })
            },
        )?;
        let mut games = records(&data)?.iter().filter(|record| {
            record["name"]
                .as_str()
                .is_some_and(|name| providers::normalized_title(name) == title)
        });
        let first = games.next().and_then(|record| record["id"].as_u64());
        Ok(first.filter(|id| games.all(|record| record["id"].as_u64() == Some(*id))))
    }

    pub fn modworkshop_page(
        &self,
        game: u64,
        query: &str,
        page: u32,
        order: CatalogOrder,
        refresh: bool,
    ) -> Result<CatalogPage, String> {
        let url = format!(
            "{API}/games/{game}/mods?page={page}&query={}{}",
            crate::catalog::encode(query),
            if order == CatalogOrder::Downloads {
                "&sort=downloads"
            } else {
                ""
            }
        );
        let data = self.modworkshop_json(&url, refresh, query.is_empty() && page == 1)?;
        let packages = records(&data)?
            .iter()
            .filter(|row| row["game_id"].as_u64() == Some(game) && row["suspended"] != true)
            .filter_map(|row| package(row).ok())
            .collect();
        let next = data["meta"]["last_page"]
            .as_u64()
            .filter(|&last| last > u64::from(page))
            .map(|_| page + 1);
        Ok(CatalogPage {
            packages,
            next,
            ..Default::default()
        })
    }

    fn modworkshop_json(&self, url: &str, refresh: bool, cached: bool) -> Result<Value, String> {
        let fetch = || {
            providers::read_json(|| {
                Ok(providers::http()?.get(url).timeout(Duration::from_secs(10)))
            })
        };
        if cached {
            self.provider_json_inner(url, refresh, true, fetch)
        } else {
            fetch()
        }
    }
}

fn records(data: &Value) -> Result<&[Value], String> {
    data["data"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "Invalid ModWorkshop catalog".into())
}

pub(super) fn package(value: &Value) -> Result<Package, String> {
    let id = value["id"].as_u64().ok_or("Missing ModWorkshop ID")?;
    let icon = value["thumbnail"]["file"]
        .as_str()
        .map(|name| format!("https://storage.modworkshop.net/mods/images/{name}"))
        .unwrap_or_default();
    let description = value["short_desc"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["desc"].as_str())
        .unwrap_or("")
        .chars()
        .take(600)
        .collect();
    Ok(Package {
        id: format!("modworkshop:{id}"),
        name: value["name"].as_str().unwrap_or("Untitled mod").into(),
        owner: value["user"]["name"].as_str().unwrap_or("").into(),
        description,
        version: value["custom_version"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Latest file")
            .into(),
        page: format!("https://modworkshop.net/mod/{id}"),
        icon,
        downloads: value["downloads"].as_u64().unwrap_or(0),
        deprecated: false,
        categories: value["category"]["name"]
            .as_str()
            .map(|s| vec![s.into()])
            .unwrap_or_default(),
        source: "ModWorkshop".into(),
    })
}

impl Manager {
    pub(super) fn modworkshop_details(&self, id: &str) -> Result<crate::details::Details, String> {
        use crate::details::{Details, Media, ModMetadata, ModPost, display_date, rich};
        let id = id
            .strip_prefix("modworkshop:")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or("Invalid ModWorkshop mod ID")?;
        let url = format!("{API}/mods/{id}");
        let data = self.modworkshop_json(&url, false, true)?;
        let package = package(&data)?;
        if package.id != format!("modworkshop:{id}") || data["suspended"] == true {
            return Err("ModWorkshop item is unavailable".into());
        }
        let field = |key: &str| data[key].as_str().unwrap_or_default();
        let mut markdown = field("desc").to_owned();
        if !field("instructions").trim().is_empty() {
            markdown.push_str("\n\n## Installation\n\n");
            markdown.push_str(field("instructions"));
        }
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(&markdown));
        let media = data["images"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|image| {
                let file = image["file"].as_str()?;
                (image["visible"] != false
                    && !file.is_empty()
                    && file
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
                .then(|| Media {
                    url: format!("https://storage.modworkshop.net/mods/images/{file}"),
                    ..Default::default()
                })
            })
            .take(32)
            .collect();
        let comments = if data["comments_disabled"] == true {
            Vec::new()
        } else {
            self.modworkshop_json(&format!("{url}/comments"), false, true)
                .ok()
                .and_then(|data| {
                    Some(
                        records(&data)
                            .ok()?
                            .iter()
                            .filter_map(|row| {
                                let (text, links) = crate::details::comments::markdown_text(
                                    row["content"].as_str()?,
                                    &package.page,
                                );
                                (!text.is_empty()).then(|| ModPost {
                                    text,
                                    links,
                                    author: row["user"]["name"].as_str().unwrap_or_default().into(),
                                    date: display_date(
                                        row["created_at"].as_str().unwrap_or_default(),
                                    ),
                                    ..Default::default()
                                })
                            })
                            .take(10)
                            .collect(),
                    )
                })
                .unwrap_or_default()
        };
        let changelog = (!field("changelog").trim().is_empty())
            .then(|| ModPost {
                date: display_date(field("bumped_at")),
                text: providers::plain_text(field("changelog")),
                ..Default::default()
            })
            .into_iter()
            .collect();
        Ok(Details {
            key: package.id,
            title: package.name,
            description: package.description,
            description_blocks: rich::parse(&html, &package.page),
            developers: (!package.owner.is_empty())
                .then_some(package.owner)
                .into_iter()
                .collect(),
            genres: package.categories,
            released: display_date(data["published_at"].as_str().unwrap_or(field("created_at"))),
            store_url: Some(package.page),
            media,
            mod_metadata: Some(ModMetadata {
                version: data["version"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(field("custom_version"))
                    .into(),
                last_updated: display_date(field("updated_at")),
                likes: data["likes"].as_u64(),
                downloads: data["downloads"].as_u64(),
                views: data["views"].as_u64(),
                changelog,
                comments,
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

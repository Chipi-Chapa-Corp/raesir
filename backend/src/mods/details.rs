use super::*;
use crate::details::{Details, ModMetadata, ModPost, display_date};

impl Manager {
    pub fn details(&self, game: &LibraryGame, id: &str) -> Result<Details, String> {
        if id.starts_with("gamebanana:") {
            return self.gamebanana_details(id);
        }
        if id.starts_with("workshop:") {
            return self.workshop_details(game, id);
        }
        if id.starts_with("modworkshop:") {
            return self.modworkshop_details(id).or_else(|error| {
                if recipes::for_game(game).is_some() {
                    self.noita_details(game, id)
                } else {
                    Err(error)
                }
            });
        }
        if recipes::for_game(game).is_some() {
            return self.noita_details(game, id);
        }
        if stardew::matches(game) {
            return self.stardew_details(game, id);
        }
        let slug = community(game).ok_or("Thunderstore does not list this game")?;
        let (index, _) = self.catalog(slug, false)?;
        let package = index
            .get_package_by_full_name(id)
            .ok_or("This mod is no longer listed on Thunderstore")?;
        let version = package
            .latest_version()
            .ok_or("This mod has no available version")?;
        let (owner, name) = id.split_once('-').ok_or("Invalid mod identifier")?;
        let number = version.version_number.clone().unwrap_or_default();
        let page = format!("https://thunderstore.io/c/{slug}/p/{owner}/{name}/");
        let url = format!(
            "https://thunderstore.io/api/experimental/package/{}/{}/{}/readme/",
            crate::catalog::encode(owner),
            crate::catalog::encode(name),
            crate::catalog::encode(&number)
        );
        let path = self
            .cache
            .join(slug)
            .join("readmes")
            .join(format!("{}.json", launch::hash(url.as_bytes())));
        let cached: Option<serde_json::Value> = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let readme = cached.or_else(|| {
            if crate::http::network_disabled() {
                return None;
            }
            let fetch = || -> Result<serde_json::Value, String> {
                let response = crate::http::client()?
                    .get(&url)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| e.to_string())?;
                let bytes = crate::util::read_limited(response, 2 * 1024 * 1024)
                    .map_err(|e| e.to_string())?
                    .ok_or("Mod description is too large")?;
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if !value["markdown"].is_string() {
                    return Err("Mod description is unavailable".into());
                }
                let _ = crate::storage::write_json(&path, &value);
                Ok(value)
            };
            fetch().ok()
        });
        let summary = version.description.clone().unwrap_or_default();
        let markdown = readme
            .as_ref()
            .and_then(|v| v["markdown"].as_str())
            .unwrap_or(&summary);
        let mut html = String::new();
        pulldown_cmark::html::push_html(
            &mut html,
            pulldown_cmark::Parser::new_ext(
                markdown,
                pulldown_cmark::Options::ENABLE_TABLES
                    | pulldown_cmark::Options::ENABLE_STRIKETHROUGH,
            ),
        );
        let changelog_url = url.replace("/readme/", "/changelog/");
        let changelog = self
            .provider_json_inner(&changelog_url, false, true, || {
                providers::read_json(|| {
                    Ok(providers::http()?
                        .get(&changelog_url)
                        .timeout(Duration::from_secs(6)))
                })
            })
            .ok()
            .and_then(|value| {
                let text = value["markdown"].as_str()?.trim();
                (!text.is_empty()).then(|| ModPost {
                    text: text.into(),
                    ..Default::default()
                })
            })
            .into_iter()
            .collect();
        let downloads = package
            .versions
            .iter()
            .filter_map(|version| version.downloads.map(u64::from))
            .reduce(|total, count| total.saturating_add(count));
        Ok(Details {
            key: format!("{}:{id}", game.key),
            title: name.replace('_', " "),
            store_url: Some(page.clone()),
            description: summary,
            description_blocks: crate::details::rich::parse(&html, &page),
            description_html: Some(html),
            developers: vec![owner.into()],
            genres: package.categories.clone(),
            released: display_date(&package.date_created.date().to_string()),
            mod_metadata: Some(ModMetadata {
                version: number,
                last_updated: display_date(&package.date_updated.date().to_string()),
                likes: package.rating_score.map(u64::from),
                downloads,
                changelog,
                ..Default::default()
            }),
            updated: crate::util::unix_now(),
            ..Default::default()
        })
    }
}

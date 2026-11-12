use super::*;
use crate::details::Details;
use std::io::Read;

impl Manager {
    pub fn details(&self, game: &LibraryGame, id: &str) -> Result<Details, String> {
        if recipes::for_game(game).is_some() {
            return self.noita_details(game, id);
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
            if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
                return None;
            }
            let fetch = || -> Result<serde_json::Value, String> {
                let client = reqwest::blocking::Client::builder()
                    .user_agent("ExposedGames/0.1")
                    .connect_timeout(Duration::from_secs(3))
                    .timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                client
                    .get(&url)
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| e.to_string())?
                    .take(2 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err("Mod description is too large".into());
                }
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
        Ok(Details {
            key: format!("{}:{id}", game.key),
            title: name.replace('_', " "),
            store_url: Some(page.clone()),
            description: summary,
            description_blocks: crate::details::rich::parse(&html, &page),
            description_html: Some(html),
            developers: vec![owner.into()],
            genres: package.categories.clone(),
            released: version.date_created.date().to_string(),
            updated: crate::details::now(),
            ..Default::default()
        })
    }
}

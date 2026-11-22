use super::{Manager, nexus, providers};
use crate::details::{Details, Media, ModMetadata, ModRequirement, display_date, rich, safe_url};
use bbscope::{BBCode, BBCodeTagConfig, ScopeInfo};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

#[derive(Deserialize)]
struct Mod {
    game_id: String,
    name: String,
    author: Option<String>,
    description: Option<String>,
    version: Option<String>,
    category: Option<Category>,
    #[serde(default)]
    gallery_images: Vec<GalleryImage>,
}

#[derive(Deserialize)]
struct Category {
    name: String,
}

#[derive(Deserialize)]
struct GalleryImage {
    url: String,
    thumbnail_url: Option<String>,
    title: Option<String>,
}

impl Manager {
    pub fn nexus_details(&self, domain: &str, id: &str) -> Result<Details, String> {
        let id = nexus::package_id(id)?;
        let url = format!("https://api.nexusmods.com/v3/games/{domain}/mods/{id}");
        let response = self.provider_json_inner(&url, false, true, || {
            providers::read_json(|| {
                Ok(nexus::nexus_headers(providers::http()?.get(&url))
                    .timeout(Duration::from_secs(10)))
            })
        })?;
        let data: Mod = serde_json::from_value(response["data"].clone())
            .map_err(|_| "Nexus Mods returned incomplete mod details")?;
        let extra = self
            .nexus_detail_metadata(&data.game_id, id)
            .unwrap_or_default();
        Ok(decode_details(data, &extra, domain, id))
    }

    fn nexus_detail_metadata(&self, game: &str, id: u64) -> Result<Value, String> {
        let body = json!({
            "query": "query Mod($game: ID!, $id: ID!) { mod(gameId: $game, modId: $id) { summary createdAt updatedAt modRequirements { nexusRequirements(count: 30) { nodes { gameId modId modName url notes } } dlcRequirements { gameExpansion { name } notes } } } }",
            "variables": {"game": game, "id": id.to_string()}
        });
        self.provider_json_inner(&format!("{}\n{body}", nexus::GRAPHQL), false, true, || {
            let value =
                providers::read_json(
                    || Ok(nexus::graphql(&body)?.timeout(Duration::from_secs(6))),
                )?;
            value["data"]["mod"]
                .as_object()
                .map(|node| Value::Object(node.clone()))
                .ok_or_else(|| "Nexus Mods returned incomplete metadata".into())
        })
    }
}

fn decode_details(data: Mod, extra: &Value, domain: &str, id: u64) -> Details {
    let page = nexus::mod_url(domain, id);
    let markup = data
        .description
        .as_deref()
        .map(description_html)
        .unwrap_or_default();
    let descriptions = rich::parse(&markup, &page);
    let mut media = data
        .gallery_images
        .into_iter()
        .filter_map(|image| {
            let url = nexus::image_url(Some(image.url))?;
            let preview = nexus::image_url(image.thumbnail_url).unwrap_or_else(|| url.clone());
            Some(Media {
                url,
                preview,
                title: image.title.unwrap_or_default(),
                video: false,
            })
        })
        .collect::<Vec<_>>();
    let mut visible = std::collections::HashSet::new();
    media.retain(|m| visible.insert(m.url.clone()));
    let summary = extra["summary"].as_str().unwrap_or_default();
    Details {
        key: format!("{domain}:nexus:{id}"),
        title: data.name,
        store_url: Some(page),
        description: summary.into(),
        description_blocks: descriptions,
        description_html: Some(markup),
        developers: data
            .author
            .into_iter()
            .filter(|author| !author.trim().is_empty())
            .collect(),
        genres: data
            .category
            .into_iter()
            .map(|category| category.name)
            .collect(),
        released: display_date(extra["createdAt"].as_str().unwrap_or_default()),
        media,
        mod_metadata: Some(ModMetadata {
            version: data.version.unwrap_or_default(),
            last_updated: display_date(extra["updatedAt"].as_str().unwrap_or_default()),
            requirements: mod_requirements(&extra["modRequirements"], domain, &data.game_id),
        }),
        ..Default::default()
    }
}

fn mod_requirements(value: &Value, domain: &str, game_id: &str) -> Vec<ModRequirement> {
    let mut requirements = Vec::new();
    for node in value["nexusRequirements"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(name) = node["modName"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
        else {
            continue;
        };
        let url = node["url"]
            .as_str()
            .filter(|url| safe_url(url))
            .map(str::to_owned)
            .or_else(|| {
                if node["gameId"].as_str()? != game_id {
                    return None;
                }
                let id = node["modId"].as_str()?.parse::<u64>().ok()?;
                (id > 0).then(|| nexus::mod_url(domain, id))
            });
        requirements.push(ModRequirement {
            name: name.into(),
            notes: node["notes"].as_str().unwrap_or_default().into(),
            url,
        });
    }
    for node in value["dlcRequirements"].as_array().into_iter().flatten() {
        if let Some(name) = node["gameExpansion"]["name"].as_str() {
            requirements.push(ModRequirement {
                name: name.into(),
                notes: node["notes"].as_str().unwrap_or_default().into(),
                url: None,
            });
        }
    }
    requirements
}

fn description_html(source: &str) -> String {
    static PARSER: LazyLock<BBCode> = LazyLock::new(|| {
        let mut config = BBCodeTagConfig::extended();
        config.accepted_tags.retain(|tag| tag != "spoiler");
        let mut tags = Vec::new();
        for tag in ["size", "color", "font"] {
            BBCode::add_tagmatcher(&mut tags, tag, ScopeInfo::default(), None, None)
                .expect("static Nexus BBCode tag");
        }
        for tag in ["center", "left", "right", "spoiler"] {
            let scope = ScopeInfo::basic(Arc::new(|_, body, _| format!("<div>{body}</div>")));
            BBCode::add_tagmatcher(&mut tags, tag, scope, None, None)
                .expect("static Nexus BBCode tag");
        }
        BBCode::from_config(config, Some(tags)).expect("static BBCode parser")
    });
    // Nexus combines BBCode with HTML line breaks.
    let source = source
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace('\u{feff}', "");
    PARSER.parse(&source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_nexus_response_uses_string_game_ids_and_builds_requirement_links() {
        let data: Mod = serde_json::from_value(json!({
            "game_id": "1303", "name": "Fixture Mod", "version": "1.2",
            "gallery_images": [{"url": "https://images.nexusmods.com/fixture.png"}]
        }))
        .unwrap();
        let extra = json!({"modRequirements": {"nexusRequirements": {"nodes": [
            {"gameId": "1303", "modId": "24", "modName": "Fixture Requirement", "url": ""}
        ]}}});
        let details = decode_details(data, &extra, "stardewvalley", 45);
        assert_eq!(details.media.len(), 1);
        let metadata = details.mod_metadata.unwrap();
        assert_eq!(metadata.version, "1.2");
        assert_eq!(
            metadata.requirements[0].url.as_deref(),
            Some("https://www.nexusmods.com/stardewvalley/mods/24")
        );
    }

    #[test]
    fn nexus_description_preserves_formatting_images_and_links() {
        let source = "[size=3][b]Update[/b][/size]<br />[url=https://example.org/wiki]Read the guide[/url][img]https://images.nexusmods.com/screenshot.png[/img]";
        let blocks = rich::parse(
            &description_html(source),
            "https://www.nexusmods.com/game/mods/1",
        );
        assert!(
            matches!(&blocks[0], rich::Block::Text(text) if text.spans.iter().any(|s| s.text == "Update" && s.style.bold))
        );
        assert!(
            matches!(&blocks[0], rich::Block::Text(text) if text.spans.iter().any(|s| s.text == "Read the guide" && s.style.link.as_deref() == Some("https://example.org/wiki")))
        );
        assert!(
            matches!(&blocks[1], rich::Block::Image { url, .. } if url == "https://images.nexusmods.com/screenshot.png")
        );
    }
}

use crate::{catalog, model::*};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Artwork {
    pub path: String,
    pub title: Option<String>,
}

fn stem(game: &LibraryGame) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    game.key.hash(&mut hash);
    game.cover.hash(&mut hash);
    if game.store == "xbox" {
        "portrait-v2".hash(&mut hash);
    }
    format!("{:016x}", hash.finish())
}
pub fn cached(game: &LibraryGame, root: &Path) -> Option<Artwork> {
    let stem = stem(game);
    let path = root.join(format!("{stem}.img"));
    if !path.is_file() {
        return None;
    }
    let title = fs::read(root.join(format!("{stem}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("title").and_then(|v| v.as_str()).map(str::to_owned));
    Some(Artwork {
        path: path.to_string_lossy().into(),
        title,
    })
}
pub fn fetch(game: &LibraryGame, root: &Path) -> Result<Artwork, String> {
    if let Some(path) = game.cover.as_ref().filter(|p| Path::new(p).is_file()) {
        return Ok(Artwork {
            path: path.clone(),
            title: None,
        });
    }
    if let Some(value) = cached(game, root) {
        return Ok(value);
    }
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
        return Err("Artwork networking disabled for diagnostics".into());
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let key = stem(game);
    let missing = root.join(format!("{key}.missing"));
    if fs::metadata(&missing)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|d| d < Duration::from_secs(86400))
    {
        return Err("Artwork is not available yet".into());
    }
    static HTTP: std::sync::LazyLock<Result<reqwest::blocking::Client, String>> =
        std::sync::LazyLock::new(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(7))
                .connect_timeout(Duration::from_secs(3))
                .user_agent("Raesir/0.1")
                .build()
                .map_err(|e| e.to_string())
        });
    let http = HTTP.as_ref().map_err(Clone::clone)?;
    let mut title = None;
    let mut candidates = game.cover.iter().cloned().collect::<Vec<_>>();
    if game.store == "xbox"
        && game.id.len() == 12
        && let Ok(value) = json(
            http,
            &format!(
                "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds={}&market=US&languages=en-US",
                catalog::encode(&game.id)
            ),
        )
        && let Some(properties) = value.pointer("/Products/0/LocalizedProperties/0")
    {
        title = properties
            .get("ProductTitle")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Some(images) = properties.get("Images").and_then(|v| v.as_array())
            && let Some(image) = images
                .iter()
                .find(|v| v.get("ImagePurpose").and_then(|s| s.as_str()) == Some("Poster"))
                .or_else(|| images.first())
                .and_then(|v| v.get("Uri"))
                .and_then(|v| v.as_str())
        {
            candidates.insert(
                0,
                if image.starts_with("//") {
                    format!("https:{image}")
                } else {
                    image.into()
                },
            );
        }
    }
    if game.store == "steam" && game.id.bytes().all(|b| b.is_ascii_digit()) {
        candidates.push(format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
            game.id
        ));
    }
    for url in candidates {
        if let Ok(bytes) = image(http, &url) {
            return save(root, &key, &bytes, title.clone());
        }
    }
    // Exact title matches only: never give a card another edition's artwork.
    if game.store != "steam"
        && let Ok(data) = json(
            http,
            &format!(
                "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
                catalog::encode(&game.title)
            ),
        )
        && let Some(item) = data
            .get("items")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .find(|v| {
                v.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| normalize(s) == normalize(&game.title))
            })
        && let Some(id) = item.get("id").and_then(|v| v.as_u64())
    {
        let url =
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/library_600x900.jpg");
        if let Ok(bytes) = image(http, &url) {
            return save(root, &key, &bytes, None);
        }
    }
    let _ = fs::write(missing, []);
    Err("No matching cover artwork found".into())
}
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn json(http: &reqwest::blocking::Client, url: &str) -> Result<Value, String> {
    let response = http
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| "Artwork metadata unavailable")?;
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|_| "Artwork metadata could not be read")?;
    serde_json::from_slice(&bytes).map_err(|_| "Artwork metadata is invalid".into())
}
fn image(http: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    if Path::new(url).is_file() {
        fs::File::open(url)
            .map_err(|e| e.to_string())?
            .take(6 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    } else {
        let parsed = reqwest::Url::parse(url).map_err(|_| "Invalid artwork URL")?;
        if parsed.scheme() != "https"
            || parsed
                .host_str()
                .is_none_or(|h| h == "localhost" || h.parse::<std::net::IpAddr>().is_ok())
        {
            return Err("Unsupported artwork URL".into());
        }
        http.get(parsed)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| "Cover unavailable")?
            .take(6 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cover could not be read")?;
    }
    if bytes.len() > 6 * 1024 * 1024
        || !(bytes.starts_with(b"\xff\xd8\xff")
            || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
            || bytes.starts_with(b"GIF8"))
    {
        return Err("Invalid cover image".into());
    }
    Ok(bytes)
}
fn save(root: &Path, key: &str, bytes: &[u8], title: Option<String>) -> Result<Artwork, String> {
    let path: PathBuf = root.join(format!("{key}.img"));
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    temp.write_all(bytes).map_err(|e| e.to_string())?;
    temp.persist(&path).map_err(|e| e.to_string())?;
    if let Some(title) = &title {
        let _ = crate::storage::write_json(
            &root.join(format!("{key}.json")),
            &serde_json::json!({"title":title}),
        );
    }
    Ok(Artwork {
        path: path.to_string_lossy().into(),
        title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_cover_takes_priority_over_download_and_missing_caches() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("library_600x900.jpg");
        fs::write(&local, b"local portrait").unwrap();
        let mut game = catalog::entry("steam", "3527290", "PEAK", vec![], "steam");
        game.cover = Some(local.to_string_lossy().into_owned());
        let key = stem(&game);
        fs::write(dir.path().join(format!("{key}.missing")), []).unwrap();
        fs::write(dir.path().join(format!("{key}.img")), b"old header").unwrap();
        assert_eq!(
            fetch(&game, dir.path()).unwrap().path,
            local.to_string_lossy()
        );
    }
}

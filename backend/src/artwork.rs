use crate::{catalog, model::*};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
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
    if game.store == "steam" {
        "portrait-before-header-v3".hash(&mut hash);
    }
    if game.store == "xbox" {
        "portrait-v2".hash(&mut hash);
    }
    if game.store == "epic" {
        "epic-portrait-v1".hash(&mut hash);
    }
    if game.store == "ubisoft" {
        "localized-portrait-v1".hash(&mut hash);
    }
    if game.store == "ea" {
        "ea-portrait-v1".hash(&mut hash);
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
    fetch_cancelable(game, root, &AtomicBool::new(false))
}

pub fn fetch_cancelable(
    game: &LibraryGame,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<Artwork, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
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
    let missing = root.join(format!("{key}.lookup-v2.missing"));
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
    if game.store == "ubisoft"
        && let Some(url) = crate::adapters::ubisoft_cover(game)
        && !candidates.contains(&url)
    {
        candidates.insert(0, url);
    }
    if game.store == "xbox"
        && game.id.len() == 12
        && (game.cover.is_none() || game.title == game.id)
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
    for url in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err("Artwork request superseded".into());
        }
        if let Ok(bytes) = image(http, &url, cancel) {
            return save(root, &key, &bytes, title.clone());
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    let provider_cover = match game.store.as_str() {
        "epic" => crate::details::epic_cover(http, game).ok(),
        "ea" => crate::details::ea_cover(http, game).ok(),
        _ => None,
    };
    if let Some(url) = provider_cover
        && let Ok(bytes) = image(http, &url, cancel)
    {
        return save(root, &key, &bytes, None);
    }
    if game.store == "steam"
        && let Some(url) = steam_portrait(http, &game.id)
        && let Ok(bytes) = image(http, &url, cancel)
    {
        return save(root, &key, &bytes, None);
    }
    // Header capsules are landscape fallbacks, after both portrait sources.
    if game.store == "steam"
        && game.id.bytes().all(|b| b.is_ascii_digit())
        && let Ok(bytes) = image(
            http,
            &format!(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
                game.id
            ),
            cancel,
        )
    {
        return save(root, &key, &bytes, None);
    }
    if game.store == "steam"
        && let Ok(data) = json(
            http,
            &format!(
                "https://store.steampowered.com/api/appdetails?appids={}",
                catalog::encode(&game.id)
            ),
        )
        && let Some(url) = data
            .get(&game.id)
            .and_then(|v| v.pointer("/data/header_image"))
            .and_then(Value::as_str)
        && let Ok(bytes) = image(http, url, cancel)
    {
        return save(root, &key, &bytes, None);
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
        if let Ok(bytes) = image(http, &url, cancel) {
            return save(root, &key, &bytes, None);
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    let _ = fs::write(missing, []);
    Err("No matching cover artwork found".into())
}
fn steam_portrait(http: &reqwest::blocking::Client, id: &str) -> Option<String> {
    let appid = id.parse::<u32>().ok()?;
    let input = serde_json::json!({"ids":[{"appid":appid}],"context":{"language":"english","country_code":"US"},"data_request":{"include_assets":true}});
    let data = json(
        http,
        &format!(
            "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json={}",
            catalog::encode(&input.to_string())
        ),
    )
    .ok()?;
    steam_portrait_url(&data, appid)
}

fn steam_portrait_url(data: &Value, appid: u32) -> Option<String> {
    let assets = data.pointer("/response/store_items/0/assets")?;
    let file = assets
        .get("library_capsule_2x")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            assets
                .get("library_capsule")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })?;
    let path = assets
        .get("asset_url_format")?
        .as_str()?
        .replace("${FILENAME}", file);
    path.starts_with(&format!("steam/apps/{appid}/"))
        .then(|| format!("https://shared.akamai.steamstatic.com/store_item_assets/{path}"))
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
fn thumbnail_url(url: &str) -> Result<reqwest::Url, String> {
    let mut parsed = reqwest::Url::parse(url).map_err(|_| "Invalid artwork URL")?;
    if parsed.host_str() == Some("store-images.s-microsoft.com") {
        let pairs: Vec<_> = parsed
            .query_pairs()
            .filter(|(k, _)| !matches!(k.as_ref(), "w" | "h" | "q"))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        parsed.set_query(None);
        parsed
            .query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("w", "272");
    }
    Ok(parsed)
}

fn image(
    _http: &reqwest::blocking::Client,
    url: &str,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Artwork request superseded".into());
    }
    let mut bytes = Vec::new();
    if Path::new(url).is_file() {
        fs::File::open(url)
            .map_err(|e| e.to_string())?
            .take(6 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    } else {
        let parsed = thumbnail_url(url)?;
        if parsed.scheme() != "https"
            || parsed
                .host_str()
                .is_none_or(|h| h == "localhost" || h.parse::<std::net::IpAddr>().is_ok())
        {
            return Err("Unsupported artwork URL".into());
        }
        bytes = remote_image(parsed, cancel)?;
    }
    if bytes.len() > 6 * 1024 * 1024
        || !(bytes.starts_with(b"\xff\xd8\xff")
            || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
            || bytes.starts_with(b"GIF8")
            || bytes.get(4..12) == Some(b"ftypavif")
            || bytes.get(4..12) == Some(b"ftypavis"))
    {
        return Err("Invalid cover image".into());
    }
    Ok(bytes)
}

/// Dropping a timed-out polling reference keeps the request alive; dropping the
/// owned future when cancelled aborts the connection, including a pending send.
fn remote_image(url: reqwest::Url, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    struct Transport {
        runtime: tokio::runtime::Runtime,
        client: reqwest::Client,
    }
    static HTTP: std::sync::LazyLock<Result<Transport, String>> = std::sync::LazyLock::new(|| {
        Ok(Transport {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(7))
                .connect_timeout(Duration::from_secs(3))
                .user_agent("Raesir/0.1")
                .build()
                .map_err(|e| e.to_string())?,
        })
    });
    let http = HTTP.as_ref().map_err(Clone::clone)?;
    http.runtime.block_on(async {
        let mut request = Box::pin(async {
            let mut response = http
                .client
                .get(url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|_| "Cover unavailable")?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "Cover could not be read")?
            {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > 6 * 1024 * 1024 {
                    break;
                }
            }
            Ok(bytes)
        });
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Artwork request superseded".into());
            }
            if let Ok(result) = tokio::time::timeout(Duration::from_millis(20), &mut request).await
            {
                return result;
            }
        }
    })
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
    fn obsolete_request_aborts_while_waiting_for_http_headers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://{}/pending",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let (received, wait_received) = std::sync::mpsc::channel();
        let (release, wait_release) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 2048];
            stream.read(&mut buffer).unwrap();
            received.send(()).unwrap();
            let _ = wait_release.recv_timeout(Duration::from_secs(5));
        });
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (done, wait_done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(remote_image(url, &worker_cancel)).unwrap();
        });
        wait_received.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let result = wait_done
            .recv_timeout(Duration::from_millis(500))
            .expect("cancel must not wait for HTTP timeout");
        assert!(result.unwrap_err().contains("superseded"));
        release.send(()).unwrap();
        worker.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn thumbnail_dimensions_replace_duplicate_parameters_and_cancel_does_not_poison_cache() {
        let url = thumbnail_url("https://store-images.s-microsoft.com/image/test?w=1920&h=1080&w=384&q=90&mode=letterbox").unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.iter().filter(|(k, _)| k == "w").count(), 1);
        assert!(pairs.iter().any(|(k, v)| k == "w" && v == "272"));
        assert!(!pairs.iter().any(|(k, _)| k == "h"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-created");
        let game = catalog::entry("xbox", "TEST", "Fixture", vec![], "fixture");
        assert!(fetch_cancelable(&game, &path, &AtomicBool::new(true)).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn steam_portraits_use_declared_asset_names_and_ignore_empty_high_resolution_slots() {
        let data = serde_json::json!({"response":{"store_items":[{"assets":{
            "asset_url_format":"steam/apps/319510/${FILENAME}?t=123",
            "library_capsule_2x":"", "library_capsule":"portrait.png"
        }}]}});
        let url = steam_portrait_url(&data, 319510).unwrap();
        assert!(url.ends_with("/319510/portrait.png?t=123"));
        assert!(steam_portrait_url(&data, 42).is_none());
    }

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

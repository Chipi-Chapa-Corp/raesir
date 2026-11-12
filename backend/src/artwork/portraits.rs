//! Key-free replacement covers for Steam's generated portrait placeholders.
use super::*;
use scraper::{Html, Selector};

mod launchbox;

const DAY: u64 = 86_400;
const REVISION: u8 = 3;

pub fn fallback_applies(game: &LibraryGame) -> bool {
    game.store == "steam"
        && game.id.parse::<u32>().is_ok()
        && game.cover.as_ref().is_none_or(|cover| {
            // Explicit custom artwork takes priority over automatic selection.
            let file = Path::new(cover).file_name().and_then(|s| s.to_str());
            file.is_some_and(|s| {
                s == "library_600x900.jpg"
                    || s == "library_600x900.png"
                    || s == format!("{}_library_600x900.jpg", game.id)
            }) || reqwest::Url::parse(cover).ok().is_some_and(|url| {
                url.host_str().is_some_and(|host| {
                    host.ends_with(".steamstatic.com") || host.ends_with(".steamcontent.com")
                })
            })
        })
}

pub fn cache_tag(game: &LibraryGame) -> Option<(&'static str, u8)> {
    fallback_applies(game).then_some(("public-portraits", REVISION))
}

#[derive(Serialize, Deserialize)]
struct Lookup {
    #[serde(default)]
    revision: u8,
    retry_after: u64,
    portrait: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn fetch(game: &LibraryGame, root: &Path, cancel: &AtomicBool) -> Option<Artwork> {
    fetch_using(
        game,
        root,
        cancel,
        |url| super::remote_json(url, cancel),
        |url| {
            wait_for_wiki(cancel)?;
            let url = reqwest::Url::parse(url).map_err(|_| "Invalid wiki URL")?;
            let bytes = super::remote_bytes(url, cancel, 2 * 1024 * 1024)?;
            String::from_utf8(bytes).map_err(|_| "Invalid wiki page".into())
        },
        |url| super::image(url, cancel),
    )
}

/// PCGamingWiki documents 60 API requests/minute. A redirect lookup also loads
/// the article, so leave two seconds between lookups. Waiting jobs remain
/// cancellable and occupy only the existing bounded artwork worker slots.
fn wait_for_wiki(cancel: &AtomicBool) -> Result<(), String> {
    use std::time::Instant;
    static NEXT: std::sync::LazyLock<std::sync::Mutex<Instant>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(Instant::now()));
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Artwork request superseded".into());
        }
        {
            let mut next = NEXT.lock().unwrap_or_else(|e| e.into_inner());
            if Instant::now() >= *next {
                *next = Instant::now() + Duration::from_millis(2100);
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fetch_using(
    game: &LibraryGame,
    root: &Path,
    cancel: &AtomicBool,
    request: impl FnMut(&str) -> Result<Value, String>,
    page: impl FnMut(&str) -> Result<String, String>,
    mut download: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Option<Artwork> {
    // Steam caches generated portrait.png placeholders under a .jpg filename.
    // Real cached JPEG portraits can be displayed without remote metadata.
    if let Some(path) = game.cover.as_deref().filter(|p| Path::new(p).is_file()) {
        let mut signature = [0u8; 3];
        if fs::File::open(path).ok().is_some_and(|mut file| {
            file.read_exact(&mut signature).is_ok() && signature == [0xff, 0xd8, 0xff]
        }) {
            return None;
        }
    }
    let dir = root.join("portraits");
    let stem = format!("{}-portrait-v2", game.id);
    let art = dir.join(format!("{stem}.img"));
    // Keep successful downloads from the first provider; ignore its negative
    // lookups now that a second source can supply the missing portrait.
    let legacy = root
        .join("pcgamingwiki")
        .join(format!("{}-portrait-v1.img", game.id));
    if let Some(art) = [&art, &legacy].into_iter().find(|p| p.is_file()) {
        return Some(Artwork {
            path: art.to_string_lossy().into_owned(),
            title: None,
        });
    }
    if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
        return None;
    }
    fs::create_dir_all(&dir).ok()?;
    let lookup_path = dir.join(format!("{stem}.json"));
    let old = fs::read(&lookup_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Lookup>(&b).ok());
    let lookup = match old.filter(|lookup| {
        lookup.retry_after > now() && (lookup.portrait.is_some() || lookup.revision == REVISION)
    }) {
        Some(lookup) => lookup,
        None => {
            let result = resolve(&game.id, &game.title, request, page);
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let lookup = match result {
                Ok((portrait, lifetime)) => Lookup {
                    revision: REVISION,
                    portrait,
                    retry_after: now() + lifetime,
                },
                Err(_) => Lookup {
                    revision: REVISION,
                    portrait: None,
                    retry_after: now() + 3600,
                },
            };
            let _ = crate::storage::write_json(&lookup_path, &lookup);
            lookup
        }
    };
    let url = lookup.portrait?;
    match download(&url) {
        Ok(bytes) if !cancel.load(Ordering::Relaxed) => super::save(&dir, &stem, &bytes, None).ok(),
        _ => {
            if !cancel.load(Ordering::Relaxed) {
                let _ = crate::storage::write_json(
                    &lookup_path,
                    &Lookup {
                        revision: REVISION,
                        portrait: None,
                        retry_after: now() + 3600,
                    },
                );
            }
            None
        }
    }
}

fn resolve(
    id: &str,
    title: &str,
    mut request: impl FnMut(&str) -> Result<Value, String>,
    mut page: impl FnMut(&str) -> Result<String, String>,
) -> Result<(Option<String>, u64), String> {
    let appid = id.parse::<u32>().map_err(|_| "Invalid Steam app ID")?;
    let input = serde_json::json!({"ids":[{"appid":appid}],"context":{"language":"english","country_code":"US"},"data_request":{"include_assets":true}});
    let steam = request(&format!(
        "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json={}",
        catalog::encode(&input.to_string())
    ))?;
    let item = steam
        .pointer("/response/store_items/0")
        .ok_or("Steam metadata unavailable")?;
    if item.get("appid").and_then(Value::as_u64) != Some(appid.into()) {
        return Err("Steam metadata did not match the requested app".into());
    }
    if let Some(url) = super::steam_portrait_url(&steam, appid)
        && reqwest::Url::parse(&url)
            .ok()
            .is_some_and(|u| !u.path().ends_with("/portrait.png"))
    {
        return Ok((None, 7 * DAY));
    }
    if let Ok(html) = page(&format!(
        "https://www.pcgamingwiki.com/api/appid.php?appid={appid}"
    )) && let Some(portrait) = select_portrait(&html, appid)
    {
        return Ok((Some(portrait), DAY));
    }
    Ok((launchbox::portrait(appid, title, &mut page)?, DAY))
}

fn select_portrait(html: &str, appid: u32) -> Option<String> {
    let doc = Html::parse_document(html);
    // Verify the redirected article actually lists this game, rather than a
    // search result, franchise page, or an edition returned by a loose search.
    let links = Selector::parse("#table-availability a[href]").ok()?;
    if !doc.select(&links).any(|a| {
        a.value()
            .attr("href")
            .and_then(|href| reqwest::Url::parse(href).ok())
            .is_some_and(|url| {
                url.host_str() == Some("store.steampowered.com")
                    && url.path().trim_end_matches('/') == format!("/app/{appid}")
            })
    }) {
        return None;
    }
    let selector = Selector::parse(".template-infobox-cover img").ok()?;
    let img = doc.select(&selector).next()?;
    let width = img.value().attr("width")?.parse::<u32>().ok()?;
    let height = img.value().attr("height")?.parse::<u32>().ok()?;
    if !(200..=640).contains(&width) || height <= width || height > width * 2 {
        return None;
    }
    // Use the wiki's own card-sized thumbnail, not its full-resolution original.
    let url = img.value().attr("src")?;
    let parsed = reqwest::Url::parse(url).ok()?;
    (parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("thumbnails.pcgamingwiki.com" | "images.pcgamingwiki.com")
        ))
    .then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steam(id: u32, file: Option<&str>) -> Value {
        serde_json::json!({"response":{"store_items":[{"appid":id,"assets":{
            "asset_url_format":format!("steam/apps/{id}/${{FILENAME}}?t=123"),
            "library_capsule":file,"library_capsule_2x":""
        }}]}})
    }

    fn wiki(id: u32, height: u32) -> String {
        format!(
            r#"<table id="table-availability"><tr><td><a href="https://store.steampowered.com/app/{id}/?utm_source=PCGamingWiki">Steam</a></td></tr></table>
            <table><tr><td class="template-infobox-cover"><span><a><img width="300" height="{height}" src="https://thumbnails.pcgamingwiki.com/2/22/Cover.jpg/300px-Cover.jpg"></a></span></td></tr></table>"#
        )
    }

    #[test]
    fn padded_and_missing_portraits_use_key_free_exact_app_id_lookups() {
        for id in [319510, 332800, 15170, 15370] {
            for file in [Some("portrait.png"), None] {
                let (url, _) = resolve(
                    &id.to_string(),
                    "Fixture",
                    |url| {
                        assert!(url.starts_with("https://api.steampowered.com/"));
                        Ok(steam(id, file))
                    },
                    |url| {
                        assert_eq!(
                            url,
                            format!("https://www.pcgamingwiki.com/api/appid.php?appid={id}")
                        );
                        Ok(wiki(id, 450))
                    },
                )
                .unwrap();
                assert_eq!(
                    url.as_deref(),
                    Some("https://thumbnails.pcgamingwiki.com/2/22/Cover.jpg/300px-Cover.jpg")
                );
            }
        }
    }

    #[test]
    fn designed_steam_portraits_and_wrong_app_metadata_do_not_query_the_wiki() {
        for response in [steam(346110, Some("library_600x900.jpg")), steam(42, None)] {
            let result = resolve(
                "346110",
                "ARK: Survival Evolved",
                |_| Ok(response.clone()),
                |_| panic!("no wiki request needed"),
            );
            assert!(result.is_err() || result.unwrap().0.is_none());
        }
    }

    #[test]
    fn landscape_covers_wrong_editions_and_untrusted_images_are_rejected() {
        assert!(select_portrait(&wiki(407530, 172), 407530).is_none());
        assert!(select_portrait(&wiki(332800, 450), 319510).is_none());
        assert!(select_portrait(&wiki(15170, 427), 15170).is_some());
        assert!(select_portrait(&wiki(15370, 377), 15370).is_some());
        assert!(
            select_portrait(
                &wiki(319510, 450).replace("thumbnails.pcgamingwiki.com", "unrelated.example"),
                319510
            )
            .is_none()
        );
        assert!(
            select_portrait(
                &wiki(319510, 450).replace("https://thumbnails", "http://thumbnails"),
                319510
            )
            .is_none()
        );
    }

    #[test]
    fn downloaded_portraits_are_reused_without_a_network_request() {
        let root = tempfile::tempdir().unwrap();
        let game = catalog::entry("steam", "319510", "FNAF", vec![], "fixture");
        let cancel = AtomicBool::new(false);
        let art = fetch_using(
            &game,
            root.path(),
            &cancel,
            |_| Ok(steam(319510, Some("portrait.png"))),
            |_| Ok(wiki(319510, 450)),
            |_| Ok(b"fixture portrait".to_vec()),
        )
        .unwrap();
        assert_eq!(fs::read(&art.path).unwrap(), b"fixture portrait");
        let cached = fetch_using(
            &game,
            root.path(),
            &cancel,
            |_| panic!("no metadata request"),
            |_| panic!("no wiki request"),
            |_| panic!("no download"),
        )
        .unwrap();
        assert_eq!(cached.path, art.path);
    }

    #[test]
    fn outages_back_off_but_cancellation_does_not_poison_lookups() {
        let root = tempfile::tempdir().unwrap();
        let game = catalog::entry("steam", "319510", "FNAF", vec![], "fixture");
        let cancel = AtomicBool::new(false);
        assert!(
            fetch_using(
                &game,
                root.path(),
                &cancel,
                |_| Err("unavailable".into()),
                |_| unreachable!(),
                |_| unreachable!()
            )
            .is_none()
        );
        assert!(
            fetch_using(
                &game,
                root.path(),
                &cancel,
                |_| panic!("cooldown"),
                |_| unreachable!(),
                |_| unreachable!()
            )
            .is_none()
        );
        let root = tempfile::tempdir().unwrap();
        assert!(
            fetch_using(
                &game,
                root.path(),
                &cancel,
                |_| {
                    cancel.store(true, Ordering::Relaxed);
                    Err("cancelled".into())
                },
                |_| unreachable!(),
                |_| unreachable!()
            )
            .is_none()
        );
        assert_eq!(
            fs::read_dir(root.path().join("portraits")).unwrap().count(),
            0
        );
        assert!(wait_for_wiki(&cancel).unwrap_err().contains("superseded"));
    }

    #[test]
    fn explicit_custom_art_and_other_stores_keep_their_existing_identity() {
        let mut game = catalog::entry("steam", "319510", "FNAF", vec![], "fixture");
        assert!(fallback_applies(&game));
        assert!(cache_tag(&game).is_some());
        game.cover = Some("/tmp/my-cover.png".into());
        assert!(!fallback_applies(&game));
        assert!(cache_tag(&game).is_none());
        game.store = "epic".into();
        game.cover = None;
        assert!(!fallback_applies(&game));
    }

    #[test]
    fn real_cached_jpeg_portraits_never_wait_for_remote_metadata() {
        let root = tempfile::tempdir().unwrap();
        let mut game = catalog::entry("steam", "346110", "ARK", vec![], "fixture");
        let path = root.path().join("library_600x900.jpg");
        fs::write(&path, b"\xff\xd8\xfffixture jpeg").unwrap();
        game.cover = Some(path.to_string_lossy().into_owned());
        assert!(
            fetch_using(
                &game,
                root.path(),
                &AtomicBool::new(false),
                |_| panic!("no metadata request"),
                |_| panic!("no wiki request"),
                |_| panic!("no image download")
            )
            .is_none()
        );
    }

    #[test]
    fn launchbox_fills_the_portrait_gap_after_a_wide_or_unavailable_wiki_cover() {
        for unavailable in [false, true] {
            let (portrait, _) = resolve("407530", "ARK: Survival Of The Fittest",
                |_| Ok(steam(407530, Some("portrait.png"))),
                |url| {
                    if url.contains("pcgamingwiki.com") {
                        return if unavailable { Err("wiki unavailable".into()) } else { Ok(wiki(407530, 172)) };
                    }
                    if url.contains("/games/results/") {
                        return Ok(r#"<div id="cardsContainer"><a href="/games/details/49766-ark"><h3>ARK: Survival of the Fittest</h3></a></div>"#.into());
                    }
                    assert_eq!(url, "https://gamesdb.launchbox-app.com/games/details/49766-ark");
                    Ok(r#"<a href="https://store.steampowered.com/app/407530">Steam</a><img alt="ARK - Box - Front (North America) - 600x900" src="https://images.launchbox-app.com/portrait.png">"#.into())
                }).unwrap();
            assert_eq!(
                portrait.as_deref(),
                Some("https://images.launchbox-app.com/portrait.png")
            );
        }
    }

    #[test]
    fn adding_a_provider_preserves_old_downloads_but_drops_old_negative_lookups() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("pcgamingwiki");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join("407530-portrait-v1.json"),
            serde_json::to_vec(&Lookup {
                revision: 0,
                portrait: None,
                retry_after: now() + DAY,
            })
            .unwrap(),
        )
        .unwrap();
        let game = catalog::entry("steam", "407530", "ARK", vec![], "fixture");
        let cancel = AtomicBool::new(false);
        let calls = std::cell::Cell::new(0);
        assert!(
            fetch_using(
                &game,
                root.path(),
                &cancel,
                |_| {
                    calls.set(calls.get() + 1);
                    Err("offline fixture".into())
                },
                |_| unreachable!(),
                |_| unreachable!()
            )
            .is_none()
        );
        assert_eq!(
            calls.get(),
            1,
            "old negative result must not suppress new providers"
        );
        let path = old.join("407530-portrait-v1.img");
        fs::write(&path, b"already downloaded cover").unwrap();
        let art = fetch_using(
            &game,
            root.path(),
            &cancel,
            |_| panic!("reuse prior download"),
            |_| unreachable!(),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(Path::new(&art.path), path);
    }

    #[test]
    fn stale_provider_misses_are_retried_after_a_parser_update() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("portraits");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("729720-portrait-v2.json"),
            serde_json::to_vec(&serde_json::json!({
                "retry_after": now() + DAY, "portrait": null
            }))
            .unwrap(),
        )
        .unwrap();
        let game = catalog::entry("steam", "729720", "Zomborg", vec![], "fixture");
        let calls = std::cell::Cell::new(0);
        fetch_using(
            &game,
            root.path(),
            &AtomicBool::new(false),
            |_| {
                calls.set(calls.get() + 1);
                Err("offline fixture".into())
            },
            |_| unreachable!(),
            |_| unreachable!(),
        );
        assert_eq!(calls.get(), 1);
    }
}

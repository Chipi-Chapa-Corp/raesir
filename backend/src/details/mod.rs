use crate::model::{ActionTarget, LibraryGame};
use reqwest::blocking::{Client, RequestBuilder};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};
pub mod comments;
pub mod media;
mod providers;
pub(crate) use providers::{ea_cover, epic_cover, steam_parent, steam_product};
pub mod reviews;
pub mod rich;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Details {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub store_url: Option<String>,
    pub description: String,
    #[serde(default)]
    pub description_html: Option<String>,
    #[serde(skip)]
    pub description_blocks: Vec<rich::Block>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub genres: Vec<String>,
    pub released: String,
    pub age_rating: String,
    pub scores: Vec<String>,
    pub media: Vec<Media>,
    pub requirements: Vec<Requirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_metadata: Option<ModMetadata>,
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub steam_reviews: Option<reviews::SteamReviews>,
    pub review_summary: String,
    pub review_source: String,
    pub sources: Vec<String>,
    pub warnings: Vec<String>,
    pub updated: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModMetadata {
    pub version: String,
    pub last_updated: String,
    pub requirements: Vec<ModRequirement>,
    #[serde(default)]
    pub likes: Option<u64>,
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub changelog: Vec<ModPost>,
    #[serde(default)]
    pub comments: Vec<ModPost>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModPost {
    pub title: String,
    pub author: String,
    pub date: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<comments::CommentLink>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModRequirement {
    pub name: String,
    pub notes: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Media {
    pub url: String,
    pub preview: String,
    pub title: String,
    pub video: bool,
}
impl Media {
    pub fn thumbnail_url(&self) -> &str {
        if self.video || !self.preview.is_empty() {
            &self.preview
        } else {
            &self.url
        }
    }

    pub fn poster_url(&self) -> String {
        let source = if self.video { &self.preview } else { &self.url };
        if let Some(url) = crate::artwork::xbox_viewer_url(source) {
            return url.into();
        }
        if self.video
            && let Ok(mut url) = reqwest::Url::parse(source)
            && url
                .host_str()
                .is_some_and(|host| host.ends_with(".steamstatic.com"))
            && matches!(
                url.path().rsplit('/').next(),
                Some(
                    "movie.293x165.jpg"
                        | "movie_293x165.jpg"
                        | "movie.600x337.jpg"
                        | "movie_600x337.jpg"
                        | "movie.184x123.jpg"
                )
            )
        {
            let path = format!("{}/movie_full.jpg", url.path().rsplit_once('/').unwrap().0);
            url.set_path(&path);
            return url.into();
        }
        source.clone()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Requirement {
    pub platform: String,
    pub minimum: String,
    pub recommended: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Review {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub steam_id: String,
    #[serde(default)]
    pub hours_at_review: Option<f64>,
    #[serde(default)]
    pub helpful: Option<u64>,
    #[serde(default)]
    pub funny: Option<u64>,
    #[serde(default)]
    pub steam_purchase: Option<bool>,
    #[serde(default)]
    pub received_free: bool,
    pub author: String,
    pub title: String,
    pub text: String,
    pub rating: String,
    pub date: String,
    #[serde(default)]
    pub hours: Option<f64>,
    #[serde(default)]
    pub platform: String,
}

impl Review {
    pub fn display_author(&self) -> &str {
        if !self.steam_id.is_empty()
            && (self.author.trim().is_empty()
                || self.author.strip_prefix("Steam user ") == Some(self.steam_id.as_str()))
        {
            "Steam reviewer"
        } else {
            &self.author
        }
    }

    fn needs_steam_author(&self) -> bool {
        !self.steam_id.is_empty() && self.display_author() == "Steam reviewer"
    }

    pub fn matches_filters(&self, since: &str, recommended: Option<bool>) -> bool {
        (since.is_empty() || self.date.as_str() >= since)
            && recommended.is_none_or(|value| self.recommended() == Some(value))
    }
    pub fn recommended(&self) -> Option<bool> {
        match self.rating.to_lowercase().as_str() {
            "recommended" => Some(true),
            "not recommended" => Some(false),
            _ => self.rating.split_once('/').and_then(|(n, max)| {
                let value = n.trim().parse::<f64>().ok()?;
                let max = max.trim().parse::<f64>().ok()?;
                (max > 0. && value.is_finite()).then_some(value / max >= 0.6)
            }),
        }
    }
}

impl Details {
    fn set_description(&mut self, source: String) {
        self.description = clean(&source);
        self.description_html = Some(source);
    }
    pub fn score(&self) -> Option<f64> {
        self.scores.iter().find_map(|s| {
            let (left, right) = s.rsplit_once('/')?;
            let value = left.split_whitespace().last()?.parse::<f64>().ok()?;
            let max = right.trim().parse::<f64>().ok()?;
            (max > 0. && value.is_finite()).then_some((value / max * 5.).clamp(0., 5.))
        })
    }
    fn normalize(&mut self) {
        let base = match self.key.split(':').next().unwrap_or_default() {
            "epic" => "https://store.epicgames.com/",
            "ea" => "https://store.ea.com/",
            "ubisoft" => "https://store.ubisoft.com/",
            "xbox" => "https://www.microsoft.com/",
            _ => "https://store.steampowered.com/",
        };
        self.description_blocks = rich::parse(
            self.description_html
                .as_deref()
                .unwrap_or(&self.description),
            base,
        );
        self.released = display_date(&self.released);
        for review in &mut self.reviews {
            review.text = review_text(&review.text);
            review.title = review.title.trim().into();
            // Older caches combined hours and platform with the author/date.
            if let Some((author, hours)) = review.author.split_once(" · ")
                && let Some(hours) = hours.strip_suffix(" hours").and_then(|s| s.parse().ok())
            {
                review.hours = Some(hours);
                review.author = author.into();
            }
            if let Some((date, platform)) = review.date.split_once(" · ") {
                review.platform = platform.into();
                review.date = date.into();
            }
            review.date = display_date(&review.date);
            review.platform = if self.review_source.starts_with("Microsoft") {
                review_platform(&review.platform)
            } else {
                String::new()
            };
        }
    }
}

fn review_text(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines().map(str::trim) {
        if !line.is_empty() || lines.last().is_some_and(|line: &&str| !line.is_empty()) {
            lines.push(line);
        }
    }
    lines.join("\n").trim().into()
}

pub fn requirement_text(text: &str) -> String {
    text.replace("OS *:", "OS:")
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !(lower.contains("steam client")
                && (lower.contains("starting january") || lower.contains("will only support")))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub fn review_cutoff(days: u32) -> String {
    if days == 0 {
        return String::new();
    }
    display_date(
        &crate::util::unix_now()
            .saturating_sub(days as u64 * 86400)
            .to_string(),
    )
}

fn review_platform(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if lower.contains("xbox") || lower == "console" {
        "Console".into()
    } else if lower.contains("windows") || lower == "pc" {
        "PC".into()
    } else {
        String::new()
    }
}

pub fn display_date(value: &str) -> String {
    use chrono::{DateTime, NaiveDate};
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<i64>() {
        return DateTime::from_timestamp(timestamp, 0)
            .map(|d| d.format("%Y.%m.%d").to_string())
            .unwrap_or_default();
    }
    let prefix = value.get(..10).unwrap_or(value);
    for (text, format) in [
        (prefix, "%Y-%m-%d"),
        (prefix, "%Y.%m.%d"),
        (value, "%b %e, %Y"),
        (value, "%e %b, %Y"),
        (value, "%B %e, %Y"),
        (value, "%e %B %Y"),
        (value, "%m/%d/%Y"),
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(text, format) {
            return date.format("%Y.%m.%d").to_string();
        }
    }
    String::new()
}

fn path(game: &LibraryGame, root: &Path) -> PathBuf {
    let mut hash = DefaultHasher::new();
    (4, &game.key, &game.title, "en-US").hash(&mut hash);
    root.join(format!("{:016x}.json", hash.finish()))
}
pub fn cached(game: &LibraryGame, root: &Path) -> Option<Details> {
    let data = fs::read(path(game, root)).ok()?;
    if data.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mut details: Details = serde_json::from_slice(&data).ok()?;
    details.normalize();
    (details.key == game.key).then_some(details)
}

pub fn refresh_cached_review_authors(game: &LibraryGame, root: &Path) -> Option<Details> {
    if crate::http::network_disabled() {
        return None;
    }
    let mut details = cached(game, root)?;
    let pages = details.steam_reviews.as_mut()?;
    if !reviews::resolve_page_authors(http().ok()?, &mut [&mut pages.main, &mut pages.recent]) {
        return None;
    }
    reviews::copy_author_names(pages, &mut details.reviews);
    let _ = crate::storage::write_json(&path(game, root), &details);
    Some(details)
}

fn store_details(
    client: &Client,
    game: &LibraryGame,
    details: &mut Details,
    with_reviews: bool,
) -> Result<(), String> {
    match game.store.as_str() {
        "epic" => providers::epic(client, game, details),
        "ea" => providers::ea(client, game, details),
        "ubisoft" => providers::ubisoft(client, game, details),
        "xbox" => providers::xbox(client, game, details),
        "wargaming" => providers::wargaming(client, game, details, with_reviews),
        _ => Err("This store does not supply game details yet.".into()),
    }
}

pub(crate) fn matching_steam_app(client: &Client, title: &str) -> Option<String> {
    let value = steam_store_search(client, title).ok()?;
    array(&value, "items")
        .iter()
        .find(|row| same_title(&text(row, "name"), title))?
        .get("id")?
        .as_u64()
        .map(|id| id.to_string())
}

impl Details {
    pub fn first_screenshot(&self) -> Option<&Media> {
        self.media
            .iter()
            .find(|media| !media.video && !media.url.is_empty())
    }

    fn has_metadata_gaps(&self) -> bool {
        self.description.is_empty()
            || self.developers.is_empty()
            || self.publishers.is_empty()
            || self.genres.is_empty()
            || self.released.is_empty()
            || self.age_rating.is_empty()
            || self.scores.is_empty()
            || self.media.is_empty()
            || self.requirements.is_empty()
            || self.reviews.is_empty()
            || self.store_url.is_none()
    }

    fn inherit_missing(&mut self, other: Self, original_edition: bool) {
        macro_rules! fill {
            ($field:ident) => {
                if self.$field.is_empty() {
                    self.$field = other.$field;
                }
            };
        }
        if self.description.is_empty() {
            self.description = other.description;
            self.description_html = other.description_html;
            self.description_blocks = other.description_blocks;
        }
        fill!(developers);
        fill!(publishers);
        fill!(genres);
        fill!(released);
        fill!(age_rating);
        fill!(scores);
        if original_edition {
            let screenshot_missing = self.first_screenshot().is_none();
            let video_missing = !self.media.iter().any(|media| media.video);
            self.media.extend(other.media.into_iter().filter(|media| {
                if media.video {
                    video_missing
                } else {
                    screenshot_missing
                }
            }));
        } else if self.media.is_empty() || !self.media.iter().any(direct_video) {
            self.media.extend(other.media);
        }
        fill!(requirements);
        if self.reviews.is_empty() && !other.reviews.is_empty() {
            self.reviews = other.reviews;
            self.review_summary = other.review_summary;
            self.review_source = other.review_source;
            self.steam_reviews = other.steam_reviews;
        }
        if original_edition && self.store_url.is_none() {
            self.store_url = other.store_url;
        }
        self.sources.push(if original_edition {
            "Additional information from the original Steam edition".into()
        } else {
            "Additional information from the matching Steam edition".into()
        });
    }
}

pub(crate) fn store_screenshot(client: &Client, game: &LibraryGame) -> Option<String> {
    let mut details = Details::default();
    if let Some(id) = (game.store == "wargaming")
        .then(|| crate::adapters::wargaming_steam_app(&game.id))
        .flatten()
    {
        providers::steam(client, id, &mut details, false).ok()?;
    } else {
        store_details(client, game, &mut details, false).ok()?;
    }
    details.first_screenshot().map(|media| media.url.clone())
}

fn has_any_content(details: &Details) -> bool {
    !(details.description.is_empty()
        && details.media.is_empty()
        && details.requirements.is_empty()
        && details.reviews.is_empty()
        && details.developers.is_empty()
        && details.publishers.is_empty()
        && details.genres.is_empty()
        && details.released.is_empty()
        && details.age_rating.is_empty()
        && details.scores.is_empty())
}

fn finalize_media_and_reviews(details: &mut Details) {
    let mut seen = std::collections::HashSet::new();
    details
        .media
        .retain(|m| safe_url(&m.url) && seen.insert(m.url.clone()));
    details.media.sort_by_key(|m| {
        if direct_video(m) {
            0
        } else if m.video {
            1
        } else {
            2
        }
    });
    details.media.truncate(24);
    details.reviews.truncate(20);
}

/// Phase 1: fetch primary store metadata quickly without waiting for reviews or
/// secondary fallback searches, so the details header, description, and media
/// render immediately.
pub fn fetch_core(game: &LibraryGame, root: &Path) -> Result<(Details, Option<String>), String> {
    if crate::http::network_disabled() {
        return Err("Game details are offline. Cached information is still available.".into());
    }
    let client = http()?;
    let mut details = Details {
        key: game.key.clone(),
        title: game.title.clone(),
        ..Default::default()
    };
    let mut parent = None;
    let result = if game.store == "steam" {
        parent = providers::steam_parent(&game.id, None);
        providers::steam(client, &game.id, &mut details, false).map(|discovered_parent| {
            parent = discovered_parent.or(parent.take());
        })
    } else {
        store_details(client, game, &mut details, false)
    };
    details.developers.retain(|s| !s.trim().is_empty());
    details.publishers.retain(|s| !s.trim().is_empty());
    if let Err(error) = result {
        details.warnings.push(error);
    }
    if !has_any_content(&details) {
        return Err(details
            .warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "No game details were found for this edition.".into()));
    }
    // Keep any previously cached reviews visible while Phase 2 refreshes them.
    if details.reviews.is_empty()
        && let Some(previous) = cached(game, root)
        && !previous.reviews.is_empty()
    {
        details.reviews = previous.reviews;
        details.review_summary = previous.review_summary;
        details.review_source = previous.review_source;
        details.steam_reviews = previous.steam_reviews;
    }
    finalize_media_and_reviews(&mut details);
    details.normalize();
    Ok((details, parent))
}

/// Phase 2: fetch reviews, reviewer names, and secondary Steam fallback metadata,
/// then persist the completed details record to disk.
pub fn enrich(
    game: &LibraryGame,
    mut details: Details,
    mut parent: Option<String>,
    root: &Path,
) -> Result<Details, String> {
    if crate::http::network_disabled() {
        return Err("Game details are offline. Cached information is still available.".into());
    }
    let client = http()?;
    if game.store == "steam" {
        providers::steam_reviews(client, &game.id, &mut details);
    } else if game.store == "wargaming"
        && let Some(id) = crate::adapters::wargaming_steam_app(&game.id)
    {
        providers::steam_reviews(client, id, &mut details);
    }
    if game.store != "steam" && details.has_metadata_gaps() {
        parent = matching_steam_app(client, &game.title);
    }
    if let Some(id) = parent.filter(|_| details.has_metadata_gaps()) {
        let mut original = Details::default();
        if providers::steam(client, &id, &mut original, details.reviews.is_empty()).is_ok() {
            details.inherit_missing(original, game.store == "steam");
        }
    }
    if !has_any_content(&details) {
        return Err(details
            .warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "No game details were found for this edition.".into()));
    }
    finalize_media_and_reviews(&mut details);
    if let Some(pages) = details.steam_reviews.as_mut()
        && pages.needs_author_names()
        && let Some(previous) = cached(game, root).and_then(|data| data.steam_reviews)
        && previous.appid == pages.appid
    {
        reviews::copy_author_names(&previous, &mut pages.main.reviews);
        reviews::copy_author_names(&previous, &mut pages.recent.reviews);
        if details.review_source == "Steam players" {
            reviews::copy_author_names(pages, &mut details.reviews);
        }
    }
    details.normalize();
    details.updated = crate::util::unix_now();
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    crate::storage::write_json(&path(game, root), &details)?;
    Ok(details)
}

pub fn fetch(game: &LibraryGame, root: &Path) -> Result<Details, String> {
    match fetch_core(game, root) {
        Ok((core, parent)) => enrich(game, core, parent, root),
        Err(core_error) => {
            if crate::http::network_disabled() {
                return Err(core_error);
            }
            // Parent-only Steam editions (e.g. Unstable branches) have no own store
            // page and rely entirely on parent inheritance in Phase 2.
            let parent = (game.store == "steam")
                .then(|| providers::steam_parent(&game.id, None))
                .flatten();
            let details = Details {
                key: game.key.clone(),
                title: game.title.clone(),
                warnings: vec![core_error],
                ..Default::default()
            };
            enrich(game, details, parent, root)
        }
    }
}
fn direct_video(media: &Media) -> bool {
    media.video && !media.url.contains("youtube.com/") && !media.url.contains("youtu.be/")
}

fn http() -> Result<&'static Client, String> {
    static HTTP: crate::http::Shared =
        LazyLock::new(|| crate::http::build(Client::builder(), Duration::from_secs(12)));
    crate::http::shared(&HTTP)
}
fn response(request: RequestBuilder) -> Result<Vec<u8>, String> {
    let response = request
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            format!(
                "Store information is unavailable ({})",
                e.status()
                    .map(|s| s.as_u16().to_string())
                    .unwrap_or_else(|| "connection error".into())
            )
        })?;
    crate::util::read_limited(response, 8 * 1024 * 1024)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Store response is too large".into())
}
fn get_json(client: &Client, url: &str) -> Result<Value, String> {
    serde_json::from_slice(&response(client.get(url))?)
        .map_err(|_| "Store returned invalid information".into())
}
pub(crate) fn steam_store_search(client: &Client, title: &str) -> Result<Value, String> {
    get_json(
        client,
        &format!(
            "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
            crate::catalog::encode(title)
        ),
    )
}
fn post_json(client: &Client, url: &str, body: Value) -> Result<Value, String> {
    serde_json::from_slice(&response(client.post(url).json(&body))?)
        .map_err(|_| "Store returned invalid information".into())
}
fn get_html(client: &Client, url: &str) -> Result<String, String> {
    String::from_utf8(response(client.get(url))?).map_err(|_| "Store returned invalid text".into())
}
fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}
fn strings(v: &Value, key: &str) -> Vec<String> {
    array(v, key)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static HTML selector")
}
fn clean(s: &str) -> String {
    let document = Html::parse_fragment(s);
    let mut output = String::new();
    for node in document.tree.nodes() {
        if node.ancestors().any(|ancestor| matches!(ancestor.value(),scraper::Node::Element(e) if matches!(e.name(),"script"|"style"))) {continue;}
        match node.value() {
            scraper::Node::Text(t) => {
                output.push_str(t);
                output.push(' ');
            }
            scraper::Node::Element(e)
                if matches!(e.name(), "br" | "p" | "li" | "h2" | "h3" | "dt" | "dd") =>
            {
                output.push('\n')
            }
            _ => {}
        }
    }
    output
        .lines()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
pub(crate) fn same_title(a: &str, b: &str) -> bool {
    crate::catalog::title_key(a) == crate::catalog::title_key(b)
}
pub(crate) fn safe_url(s: &str) -> bool {
    reqwest::Url::parse(s).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str()
                .is_some_and(|h| h != "localhost" && h.parse::<std::net::IpAddr>().is_err())
    })
}
fn absolute(url: &str, base: &str) -> String {
    reqwest::Url::parse(base)
        .and_then(|b| b.join(url))
        .map(|u| u.to_string())
        .unwrap_or_default()
}
fn store_url(game: &LibraryGame) -> Option<&str> {
    game.store_page.as_ref().and_then(|a| {
        if let ActionTarget::Url { url } = &a.target {
            Some(url.as_str())
        } else {
            None
        }
    })
}
fn embedded_json(s: &str, marker: &str) -> Option<Value> {
    let value = s
        .split_once(marker)?
        .1
        .trim_start()
        .trim_start_matches('=')
        .trim_start();
    serde_json::Deserializer::from_str(value)
        .into_iter::<Value>()
        .next()?
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steam_gallery_posters_use_full_images_without_changing_strip_thumbnails() {
        for filename in ["movie.293x165.jpg", "movie_600x337.jpg"] {
            let thumbnail = format!(
                "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/256804553/{filename}?t=1602783173"
            );
            let media = Media {
                preview: thumbnail.clone(),
                video: true,
                ..Default::default()
            };
            assert_eq!(
                media.poster_url(),
                "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/256804553/movie_full.jpg?t=1602783173"
            );
            assert_eq!(media.preview, thumbnail);
        }
        let media = Media {
            preview: "https://example.com/movie.293x165.jpg".into(),
            video: true,
            ..Default::default()
        };
        assert_eq!(media.poster_url(), media.preview);
        let screenshot = Media {
            url: "https://cdn.example/screenshot.jpg".into(),
            ..Default::default()
        };
        assert_eq!(screenshot.poster_url(), screenshot.url);
    }

    #[test]
    fn xbox_gallery_uses_a_distinct_high_resolution_viewer_url() {
        let original =
            "https://store-images.s-microsoft.com/image/test?h=153&w=272&q=60&mode=letterbox";
        let screenshot = Media {
            url: original.into(),
            ..Default::default()
        };
        assert_eq!(
            screenshot.poster_url(),
            "https://store-images.s-microsoft.com/image/test?mode=letterbox&w=1920&q=90"
        );
        assert_eq!(screenshot.url, original);
        let trailer = Media {
            url: "https://example.com/trailer.m3u8".into(),
            preview: original.into(),
            video: true,
            ..Default::default()
        };
        assert_eq!(trailer.poster_url(), screenshot.poster_url());
        assert_eq!(trailer.preview, original);
    }
    #[test]
    fn dates_and_cached_reviews_use_the_same_fields_across_stores() {
        for input in [
            "2025-06-16T12:30:00Z",
            "Jun 16, 2025",
            "16 Jun, 2025",
            "06/16/2025",
            "2025.06.16",
        ] {
            assert_eq!(display_date(input), "2025.06.16");
        }
        assert_eq!(display_date("2025-02-30"), "");
        assert_eq!(display_date("Coming soon"), "");
        let mut data = Details {
            review_source: "Microsoft Store customers".into(),
            reviews: vec![Review {
                author: "Player · 12 hours".into(),
                date: "2025-06-16 · Windows.Desktop".into(),
                rating: "2/5".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        data.normalize();
        assert_eq!(data.reviews[0].author, "Player");
        assert_eq!(data.reviews[0].hours, Some(12.));
        assert_eq!(data.reviews[0].date, "2025.06.16");
        assert_eq!(data.reviews[0].platform, "PC");
        assert_eq!(data.reviews[0].recommended(), Some(false));
        data.reviews[0].rating = "3/5".into();
        assert_eq!(data.reviews[0].recommended(), Some(true));
        data.review_source = "Steam players".into();
        data.normalize();
        assert!(data.reviews[0].platform.is_empty());
    }
    #[test]
    fn reviews_trim_source_padding_and_combine_date_and_rating_filters() {
        let mut details = Details {
            reviews: vec![Review {
                text: "  Good game\n\n\n\n  ".into(),
                date: "2026-10-30".into(),
                rating: "3/5".into(),
                ..Default::default()
            }],
            scores: vec!["Players 3.5/5".into()],
            ..Default::default()
        };
        details.normalize();
        let review = &details.reviews[0];
        assert_eq!(review.text, "Good game");
        assert_eq!(review_text("First\n\n\nSecond\n\n"), "First\n\nSecond");
        assert!(review.matches_filters("2026.10.01", Some(true)));
        assert!(!review.matches_filters("2026.11.01", Some(true)));
        assert!(!review.matches_filters("", Some(false)));
        assert!(review.matches_filters("", None));
        assert_eq!(details.score(), Some(3.5));
    }
    #[test]
    fn text_is_readable_and_edition_matching_is_strict() {
        assert_eq!(
            clean("<p>A &amp; B</p><ul><li>CPU: fast</li><li>RAM: 8 GB</li></ul>"),
            "A & B\nCPU: fast\nRAM: 8 GB"
        );
        assert_eq!(
            clean("<p>Safe</p><script>bad()</script><style>hidden</style>"),
            "Safe"
        );
        assert!(same_title("Assassin’s Creed II", "Assassin's Creed II"));
        assert!(!same_title("Game Deluxe Edition", "Game"));
        assert!(same_title("Assassin's Creed II", "Assassin's Creed 2"));
        assert!(!safe_url("file:///etc/passwd"));
        assert!(!safe_url("https://127.0.0.1/private"));
        assert!(safe_url("https://store-images.s-microsoft.com/image/test"));
    }
    #[test]
    fn linked_editions_keep_their_own_metadata_and_fill_gaps() {
        let mut edition = Details {
            key: "steam:367540".into(),
            title: "Starbound - Unstable".into(),
            developers: vec!["Edition developer".into()],
            media: vec![Media {
                url: "edition.jpg".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let original = Details {
            title: "Starbound".into(),
            description: "Original description".into(),
            developers: vec!["Original developer".into()],
            publishers: vec!["Publisher".into()],
            media: vec![
                Media {
                    url: "original.jpg".into(),
                    ..Default::default()
                },
                Media {
                    url: "trailer.mp4".into(),
                    video: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        edition.inherit_missing(original, true);
        assert_eq!(edition.key, "steam:367540");
        assert_eq!(edition.title, "Starbound - Unstable");
        assert_eq!(edition.description, "Original description");
        assert_eq!(edition.developers, ["Edition developer"]);
        assert_eq!(edition.publishers, ["Publisher"]);
        assert_eq!(edition.first_screenshot().unwrap().url, "edition.jpg");
        assert_eq!(edition.media.len(), 2);
        assert_eq!(edition.media[1].url, "trailer.mp4");
        assert_eq!(steam_parent("367540", None).as_deref(), Some("211820"));
        assert_eq!(
            steam_parent("2", Some(&json!({"fullgame":{"appid":"1"}}))).as_deref(),
            Some("1")
        );
        assert_eq!(
            steam_parent("1", Some(&json!({"fullgame":{"appid":"1"}}))),
            None
        );
    }

    #[test]
    fn cached_details_reject_other_games_and_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let game = crate::catalog::entry("steam", "1", "Game", vec![], "test");
        let details = Details {
            key: game.key.clone(),
            title: game.title.clone(),
            ..Default::default()
        };
        crate::storage::write_json(&path(&game, dir.path()), &details).unwrap();
        assert_eq!(cached(&game, dir.path()).unwrap().title, "Game");
        let other = Details {
            key: "steam:2".into(),
            ..details
        };
        crate::storage::write_json(&path(&game, dir.path()), &other).unwrap();
        assert!(cached(&game, dir.path()).is_none());
        fs::write(path(&game, dir.path()), "not json").unwrap();
        assert!(cached(&game, dir.path()).is_none());
    }
}

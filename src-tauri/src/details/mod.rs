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
    time::{Duration, SystemTime, UNIX_EPOCH},
};
mod providers;
pub(crate) use providers::{ea_cover, epic_cover};
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
    pub reviews: Vec<Review>,
    pub review_summary: String,
    pub review_source: String,
    pub sources: Vec<String>,
    pub warnings: Vec<String>,
    pub updated: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Media {
    pub url: String,
    pub preview: String,
    pub title: String,
    pub video: bool,
}
impl Media {
    pub fn poster_url(&self) -> String {
        let source = if self.video { &self.preview } else { &self.url };
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
            if let Some((author, hours)) = review.author.split_once(" · ") {
                if let Some(hours) = hours.strip_suffix(" hours").and_then(|s| s.parse().ok()) {
                    review.hours = Some(hours);
                    review.author = author.into();
                }
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

pub fn review_cutoff(days: u32) -> String {
    if days == 0 {
        return String::new();
    }
    display_date(&now().saturating_sub(days as u64 * 86400).to_string())
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

fn display_date(value: &str) -> String {
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

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn path(game: &LibraryGame, root: &Path) -> PathBuf {
    let mut hash = DefaultHasher::new();
    (3, &game.key, &game.title, "en-US").hash(&mut hash);
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
pub fn fetch(game: &LibraryGame, root: &Path) -> Result<Details, String> {
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
        return Err("Game details are offline. Cached information is still available.".into());
    }
    let client = http()?;
    let mut details = Details {
        key: game.key.clone(),
        title: game.title.clone(),
        ..Default::default()
    };
    let result = match game.store.as_str() {
        "steam" => providers::steam(client, &game.id, &mut details, true),
        "epic" => providers::epic(client, game, &mut details),
        "ea" => providers::ea(client, game, &mut details),
        "ubisoft" => providers::ubisoft(client, game, &mut details),
        "xbox" => providers::xbox(client, game, &mut details),
        _ => Err("This store does not supply game details yet.".into()),
    };
    details.developers.retain(|s| !s.trim().is_empty());
    details.publishers.retain(|s| !s.trim().is_empty());
    if let Err(error) = result {
        details.warnings.push(error);
    }
    if game.store != "steam" && (details.description.is_empty() || details.reviews.is_empty()) {
        // Resolve exact editions only; never silently attach another game's reviews.
        if let Ok(value) = get_json(
            client,
            &format!(
                "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
                crate::catalog::encode(&game.title)
            ),
        ) {
            if let Some(id) = array(&value, "items")
                .iter()
                .find(|v| same_title(&text(v, "name"), &game.title))
                .and_then(|v| v.get("id"))
                .and_then(Value::as_u64)
            {
                let mut other = Details::default();
                if providers::steam(client, &id.to_string(), &mut other, true).is_ok() {
                    let enriched = details.description.is_empty()
                        || details.developers.is_empty()
                        || details.publishers.is_empty()
                        || details.genres.is_empty()
                        || details.released.is_empty()
                        || details.age_rating.is_empty()
                        || details.scores.is_empty()
                        || details.media.is_empty()
                        || details.requirements.is_empty();
                    if details.description.is_empty() {
                        details.description = other.description;
                        details.description_html = other.description_html;
                    }
                    if details.developers.is_empty() {
                        details.developers = other.developers;
                    }
                    if details.publishers.is_empty() {
                        details.publishers = other.publishers;
                    }
                    if details.released.is_empty() {
                        details.released = other.released;
                    }
                    if details.age_rating.is_empty() {
                        details.age_rating = other.age_rating;
                    }
                    if details.scores.is_empty() {
                        details.scores = other.scores;
                    }
                    if details.genres.is_empty() {
                        details.genres = other.genres;
                    }
                    if details.media.is_empty() || !details.media.iter().any(direct_video) {
                        details.media.extend(other.media);
                        details
                            .sources
                            .push("Additional media from the matching Steam edition".into());
                    }
                    if details.requirements.is_empty() {
                        details.requirements = other.requirements;
                    }
                    if details.reviews.is_empty() {
                        details.reviews = other.reviews;
                        details.review_summary = other.review_summary;
                        details.review_source = other.review_source;
                    }
                    if enriched {
                        details
                            .sources
                            .push("Additional information from the matching Steam edition".into());
                    }
                }
            }
        }
    }
    if details.description.is_empty()
        && details.media.is_empty()
        && details.requirements.is_empty()
        && details.reviews.is_empty()
    {
        return Err(details
            .warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "No game details were found for this edition.".into()));
    }
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
    details.normalize();
    details.updated = now();
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    crate::storage::write_json(&path(game, root), &details)?;
    Ok(details)
}
fn direct_video(media: &Media) -> bool {
    media.video && !media.url.contains("youtube.com/") && !media.url.contains("youtu.be/")
}

fn http() -> Result<&'static Client, String> {
    static HTTP: LazyLock<Result<Client, String>> = LazyLock::new(|| {
        Client::builder()
            .user_agent("Raesir/0.1")
            .timeout(Duration::from_secs(12))
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| e.to_string())
    });
    HTTP.as_ref().map_err(Clone::clone)
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
    let mut bytes = Vec::new();
    response
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Store response is too large".into());
    }
    Ok(bytes)
}
fn get_json(client: &Client, url: &str) -> Result<Value, String> {
    serde_json::from_slice(&response(client.get(url))?)
        .map_err(|_| "Store returned invalid information".into())
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
fn same_title(a: &str, b: &str) -> bool {
    fn normalize(s: &str) -> String {
        s.split_whitespace()
            .map(|word| {
                let word = word
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                match word.as_str() {
                    "i" => "1".into(),
                    "ii" => "2".into(),
                    "iii" => "3".into(),
                    "iv" => "4".into(),
                    "v" => "5".into(),
                    "vi" => "6".into(),
                    "vii" => "7".into(),
                    "viii" => "8".into(),
                    "ix" => "9".into(),
                    "x" => "10".into(),
                    _ => word,
                }
            })
            .collect()
    }
    normalize(a) == normalize(b)
}
fn safe_url(s: &str) -> bool {
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

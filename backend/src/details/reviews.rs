//! Steam's public reviews API, with the same defaults as the store's summary.
use super::*;
use std::collections::{HashMap, HashSet};

const AUTHOR_NAME_TTL: u64 = 6 * 60 * 60;
const AUTHOR_RETRY_DELAY: u64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub review_type: u8,
    pub purchase_type: u8,
    pub language: String,
    pub date_start: u32,
    pub date_end: u32,
    pub playtime_min: u32,
    pub playtime_max: u32,
    pub display: u8,
    pub filter_offtopic: bool,
    pub use_review_quality: bool,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            review_type: 0,
            purchase_type: 1,
            language: "english".into(),
            date_start: 0,
            date_end: 0,
            playtime_min: 0,
            playtime_max: 0,
            display: 0,
            filter_offtopic: true,
            use_review_quality: true,
        }
    }
}
impl Query {
    fn input(&self, appid: u32, recent: bool, cursor: &str) -> Value {
        let filter = if recent {
            1
        } else {
            [0, 0, 1, 2, 3][self.display.min(4) as usize]
        };
        let mut input = json!({
            "appid": appid, "filter": filter, "languages": [&self.language],
            "review_type": self.review_type, "purchase_type": self.purchase_type,
            "num_per_page": 10, "cursor": cursor, "day_range": 30,
            "filter_offtopic_activity": self.filter_offtopic,
            "use_review_quality": self.use_review_quality, "display_language": "english",
        });
        if self.date_end > 0 && self.date_end >= self.date_start {
            input["date_range_start"] = self.date_start.into();
            input["date_range_end"] = self.date_end.into();
        }
        if self.playtime_min > 0 {
            input["playtime_min_hours"] = self.playtime_min.into();
        }
        if self.playtime_max > 0 {
            input["playtime_max_hours"] = self.playtime_max.into();
        }
        input
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Page {
    pub reviews: Vec<Review>,
    pub cursor: Option<String>,
    pub summary: String,
    pub total: Option<u64>,
    pub day_range: Option<u64>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
}
impl Page {
    pub fn append(&mut self, next: Self) {
        let mut seen = self
            .reviews
            .iter()
            .map(|r| r.id.clone())
            .collect::<HashSet<_>>();
        let previous_count = self.reviews.len();
        self.reviews.extend(
            next.reviews
                .into_iter()
                .filter(|r| r.id.is_empty() || seen.insert(r.id.clone())),
        );
        self.cursor = if self.reviews.len() > previous_count {
            next.cursor
        } else {
            None
        };
        if !next.summary.is_empty() {
            self.summary = next.summary;
        }
        if next.total.is_some() {
            self.total = next.total;
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SteamReviews {
    pub appid: String,
    pub main: Page,
    pub recent: Page,
}

impl SteamReviews {
    pub fn needs_author_names(&self) -> bool {
        self.main
            .reviews
            .iter()
            .chain(&self.recent.reviews)
            .any(Review::needs_steam_author)
    }
}

pub fn copy_author_names(source: &SteamReviews, target: &mut [Review]) {
    let names = source
        .main
        .reviews
        .iter()
        .chain(&source.recent.reviews)
        .filter(|review| !review.steam_id.is_empty() && !review.needs_steam_author())
        .map(|review| (review.steam_id.as_str(), &review.author))
        .collect::<HashMap<_, _>>();
    for review in target
        .iter_mut()
        .filter(|review| review.needs_steam_author())
    {
        if let Some(name) = names.get(review.steam_id.as_str()) {
            review.author.clone_from(name);
        }
    }
}

pub fn fetch(appid: &str, query: &Query, recent: bool, cursor: &str) -> Result<Page, String> {
    let client = http()?;
    let mut page = fetch_page(client, appid, query, recent, cursor)?;
    resolve_page_authors(client, &mut [&mut page]);
    Ok(page)
}

pub(super) fn fetch_page(
    client: &Client,
    appid: &str,
    query: &Query,
    recent: bool,
    cursor: &str,
) -> Result<Page, String> {
    if crate::http::network_disabled() {
        return Err("Reviews are offline. Try again when connected.".into());
    }
    let id = appid
        .parse::<u32>()
        .map_err(|_| "Steam product ID is missing")?;
    let input = query.input(id, recent, cursor);
    let value = get_json(
        client,
        &format!(
            "https://api.steampowered.com/IUserReviewsService/GetAppReviews/v1/?input_json={}",
            crate::catalog::encode(&input.to_string())
        ),
    )?;
    parse(value.get("response").unwrap_or(&value), cursor)
}

// The Community resolver accepts one SteamID per request. Cache public names
// across the helpful/recent lists and subsequent filter and pagination requests.
pub(super) fn resolve_page_authors(client: &Client, pages: &mut [&mut Page]) -> bool {
    type NameCache = HashMap<String, (u64, Option<String>)>;
    static NAMES: LazyLock<std::sync::Mutex<NameCache>> = LazyLock::new(Default::default);
    let mut missing = HashSet::new();
    let mut changed = false;
    let mut reviewers = pages
        .iter_mut()
        .flat_map(|page| &mut page.reviews)
        .filter(|review| {
            !review.steam_id.is_empty() && review.steam_id.bytes().all(|b| b.is_ascii_digit())
        })
        .collect::<Vec<_>>();
    {
        let mut cache = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        let now = crate::util::unix_now();
        let fresh = |(updated, name): &(u64, Option<String>)| {
            let ttl = if name.is_some() {
                AUTHOR_NAME_TTL
            } else {
                AUTHOR_RETRY_DELAY
            };
            now.saturating_sub(*updated) < ttl
        };
        if cache.len() > 2048 {
            cache.retain(|_, entry| fresh(entry));
        }
        for review in reviewers
            .iter()
            .filter(|review| !review.needs_steam_author())
        {
            cache.insert(review.steam_id.clone(), (now, Some(review.author.clone())));
        }
        for review in reviewers
            .iter_mut()
            .filter(|review| review.needs_steam_author())
        {
            if let Some((_, name)) = cache.get(&review.steam_id).filter(|entry| fresh(entry)) {
                if let Some(name) = name {
                    review.author.clone_from(name);
                    changed = true;
                }
            } else {
                missing.insert(review.steam_id.clone());
            }
        }
    }
    if missing.is_empty() {
        return changed;
    }
    let missing = missing.into_iter().collect::<Vec<_>>();
    let names = std::thread::scope(|scope| {
        let jobs = missing
            .chunks(missing.len().div_ceil(4))
            .map(|ids| {
                scope.spawn(move || {
                    ids.iter()
                        .map(|id| (id, lookup_author(client, id)))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        jobs.into_iter()
            .filter_map(|job| job.join().ok())
            .flatten()
            .collect::<HashMap<_, _>>()
    });
    for review in reviewers
        .iter_mut()
        .filter(|review| review.needs_steam_author())
    {
        if let Some(Some(name)) = names.get(&review.steam_id) {
            review.author.clone_from(name);
            changed = true;
        }
    }
    let mut cache = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let now = crate::util::unix_now();
    for (id, name) in names {
        let already_resolved = cache.get(id).is_some_and(|(updated, name)| {
            name.is_some() && now.saturating_sub(*updated) < AUTHOR_NAME_TTL
        });
        if name.is_some() || !already_resolved {
            cache.insert(id.clone(), (now, name));
        }
    }
    changed
}

fn lookup_author(client: &Client, id: &str) -> Option<String> {
    let request = client
        .get("https://steamcommunity.com/actions/ajaxresolveusers")
        .query(&[("steamids", id)])
        .timeout(Duration::from_secs(4));
    let bytes = response(request).ok()?;
    let users = serde_json::from_slice(&bytes).ok()?;
    author_name(&users, id).map(str::to_owned)
}

fn author_name<'a>(users: &'a Value, id: &str) -> Option<&'a str> {
    users.as_array()?.iter().find_map(|user| {
        user["steamid"]
            .as_str()
            .filter(|candidate| *candidate == id)?;
        user["persona_name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
    })
}

fn parse(value: &Value, requested_cursor: &str) -> Result<Page, String> {
    let rows = value["reviews"]
        .as_array()
        .ok_or("Steam returned invalid reviews. Try again.")?;
    let summary = &value["query_summary"];
    let label = text(summary, "review_score_desc");
    let count = summary["total_reviews"].as_u64();
    let score = count.filter(|n| *n > 0).and_then(|n| {
        value["query_summary"]["total_positive"]
            .as_u64()
            .map(|p| p as f64 / n as f64 * 5.)
    });
    let summary = count
        .map(|n| {
            if label.is_empty() {
                format!("{n} reviews")
            } else {
                format!("{label} · {n} reviews")
            }
        })
        .unwrap_or_default();
    let cursor = value["cursor"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != requested_cursor && !rows.is_empty())
        .map(str::to_owned);
    Ok(Page {
        reviews: rows
            .iter()
            .map(|r| {
                let steam_id = text(&r["author"], "steamid");
                let name = text(&r["author"], "personaname");
                Review {
                    id: text(r, "recommendationid"),
                    author: if !name.trim().is_empty() {
                        name
                    } else {
                        "Steam reviewer".into()
                    },
                    steam_id,
                    text: review_text(&strip_bbcode(&text(r, "review"))),
                    rating: if r["voted_up"].as_bool().unwrap_or(false) {
                        "Recommended"
                    } else {
                        "Not recommended"
                    }
                    .into(),
                    date: r["timestamp_created"]
                        .as_i64()
                        .map(|n| display_date(&n.to_string()))
                        .unwrap_or_default(),
                    hours: r["author"]["playtime_forever"].as_f64().map(|n| n / 60.),
                    hours_at_review: r["author"]["playtime_at_review"].as_f64().map(|n| n / 60.),
                    helpful: r["votes_up"].as_u64(),
                    funny: r["votes_funny"].as_u64(),
                    steam_purchase: r["steam_purchase"].as_bool(),
                    received_free: r["received_for_free"].as_bool().unwrap_or(false),
                    ..Default::default()
                }
            })
            .filter(|r| !r.text.is_empty())
            .collect(),
        cursor,
        summary,
        total: value["total_matching"].as_u64(),
        day_range: value["day_range_used"].as_u64(),
        score,
        error: None,
    })
}
fn strip_bbcode(input: &str) -> String {
    let mut output = String::new();
    let mut tag = false;
    for c in input.chars() {
        match c {
            '[' => tag = true,
            ']' => tag = false,
            _ if !tag => output.push(c),
            _ => {}
        }
    }
    output
}

/// Inclusive calendar date range in UTC, for Steam's timestamp filters.
pub fn date_range(start: &str, end: &str) -> Result<(u32, u32), String> {
    let parse = |s: &str| {
        chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map_err(|_| "Use dates in YYYY-MM-DD format.")
    };
    let start = parse(start)?;
    let end = parse(end)?;
    if end < start {
        return Err("The end date must be on or after the start date.".into());
    }
    let timestamp = |d: chrono::NaiveDate, h, m, s| {
        u32::try_from(d.and_hms_opt(h, m, s).unwrap().and_utc().timestamp())
            .map_err(|_| "Choose dates between 1970 and 2106.".to_owned())
    };
    Ok((timestamp(start, 0, 0, 0)?, timestamp(end, 23, 59, 59)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_store_summary_and_recent_has_its_own_sort() {
        let q = Query::default();
        let main = q.input(1478500, false, "*");
        assert_eq!(main["filter"], 0);
        assert_eq!(main["purchase_type"], 1);
        assert_eq!(main["languages"], json!(["english"]));
        assert_eq!(main["day_range"], 30);
        assert_eq!(q.input(1478500, true, "*")["filter"], 1);
    }
    #[test]
    fn all_filters_reach_steam_and_cursor_is_preserved() {
        let q = Query {
            review_type: 2,
            purchase_type: 2,
            language: "czech".into(),
            date_start: 100,
            date_end: 200,
            playtime_min: 10,
            playtime_max: 100,
            display: 4,
            filter_offtopic: false,
            use_review_quality: false,
        };
        let input = q.input(42, false, "a+/=");
        assert_eq!(input["filter"], 3);
        assert_eq!(input["review_type"], 2);
        assert_eq!(input["purchase_type"], 2);
        assert_eq!(input["languages"], json!(["czech"]));
        assert_eq!(input["date_range_start"], 100);
        assert_eq!(input["date_range_end"], 200);
        assert_eq!(input["playtime_min_hours"], 10);
        assert_eq!(input["playtime_max_hours"], 100);
        assert_eq!(input["cursor"], "a+/=");
        assert_eq!(input["filter_offtopic_activity"], false);
        assert_eq!(input["use_review_quality"], false);
    }
    #[test]
    fn reviewers_are_resolved_by_id_and_pages_stop_and_deduplicate() {
        let value = json!({"reviews":[{"recommendationid":"1","author":{"steamid":"76561198000000000","playtime_forever":120,"playtime_at_review":90},"review":"[b]Great[/b]","timestamp_created":1750032000,"voted_up":true,"votes_up":56}],"cursor":"next","total_matching":10});
        let mut value = value;
        value["query_summary"] =
            json!({"total_reviews":2,"total_positive":1,"review_score_desc":"Mixed"});
        let mut page = parse(&value, "*").unwrap();
        assert_eq!(page.summary, "Mixed · 2 reviews");
        assert_eq!(page.score, Some(2.5));
        assert_eq!(page.reviews[0].recommended(), Some(true));
        let users = json!([{"steamid":"wrong","persona_name":"Wrong"},{"steamid":"76561198000000000","persona_name":"Alsv тest"}]);
        assert!(page.reviews[0].needs_steam_author());
        assert_eq!(
            author_name(&users, &page.reviews[0].steam_id),
            Some("Alsv тest"),
        );
        assert_eq!(page.reviews[0].text, "Great");
        assert_eq!(page.reviews[0].date, "2025.06.16");
        assert_eq!(page.reviews[0].hours, Some(2.));
        assert_eq!(page.reviews[0].hours_at_review, Some(1.5));
        page.append(parse(&value, "next").unwrap());
        assert_eq!(page.reviews.len(), 1);
        assert!(page.cursor.is_none());
        assert!(
            parse(&json!({"reviews":[],"cursor":"end"}), "*")
                .unwrap()
                .cursor
                .is_none()
        );
        assert!(parse(&json!({"error":"rate limited"}), "*").is_err());
    }

    #[test]
    fn cached_numeric_fallbacks_are_replaced_by_resolved_names() {
        let missing = Review {
            steam_id: "76561198000000000".into(),
            author: "Steam user 76561198000000000".into(),
            ..Default::default()
        };
        assert_eq!(missing.display_author(), "Steam reviewer");
        let mut pages = SteamReviews::default();
        pages.main.reviews.push(missing.clone());
        assert!(pages.needs_author_names());
        pages.main.reviews[0].author = "Rocket Pilot".into();
        assert!(!pages.needs_author_names());
        let mut saved = [missing];
        copy_author_names(&pages, &mut saved);
        assert_eq!(saved[0].display_author(), "Rocket Pilot");
        assert_eq!(saved[0].steam_id, "76561198000000000");
    }
    #[test]
    fn custom_dates_include_the_whole_end_day_and_validate() {
        let (start, end) = date_range("2026-11-01", "2026-11-01").unwrap();
        assert_eq!(end - start, 86399);
        assert!(date_range("2026-11-02", "2026-11-01").is_err());
        assert!(date_range("2026-02-30", "2026-11-01").is_err());
    }
}

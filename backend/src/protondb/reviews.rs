use super::CLIENT;
use crate::{
    details::{self, Review},
    storage,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{io::Read, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Device {
    Pc,
    SteamDeck,
    Chromebook,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Proton {
    Official,
    Ge,
    Experimental,
    Native,
    Other,
}

#[derive(Deserialize, Serialize)]
pub struct Report {
    pub review: Review,
    pub device: Device,
    pub proton: Proton,
    pub has_notes: bool,
    pub has_launch_options: bool,
}

#[derive(Default, Deserialize, Serialize)]
pub struct Page {
    pub reports: Vec<Report>,
    pub total: u64,
    pub next: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct CachedPage {
    checked: u64,
    page: Page,
}

#[derive(Deserialize)]
struct Counts {
    reports: u32,
    timestamp: u32,
}

pub fn fetch(cache: &Path, app_id: u32, page: u32) -> Result<Page, String> {
    let path = cache
        .join("protondb")
        .join("reports")
        .join(format!("{app_id}-{page}.json"));
    let previous = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CachedPage>(&bytes).ok());
    let now = crate::util::unix_now();
    if crate::http::network_disabled()
        || previous
            .as_ref()
            .is_some_and(|cached| now.saturating_sub(cached.checked) < 21600)
    {
        return previous
            .map(|cached| cached.page)
            .ok_or_else(|| "ProtonDB reviews are unavailable offline.".into());
    }
    match fetch_page(app_id, page) {
        Ok(page) => {
            let cached = CachedPage { checked: now, page };
            let _ = storage::write_json(&path, &cached);
            Ok(cached.page)
        }
        Err(error) => previous.map(|cached| cached.page).ok_or(error),
    }
}

#[cfg(test)]
#[test]
fn report_page_keeps_current_and_legacy_player_notes() {
    let response = serde_json::json!({
        "page": 1, "perPage": 40, "total": 42, "reports": [
            {
                "id": "modern", "timestamp": 1700000000,
                "contributor": {"steam": {"nickname": "Player", "playtime": 120}},
                "device": {"hardwareType": "steamDeck"},
                "responses": {"notes": {"verdict": "Worked well"}, "verdict": "yes",
                    "variant": "ge", "customProtonVersion": "GE-Proton10-1", "launchOptions": "-windowed"}
            },
            {"id": "legacy", "responses": {"notes": {"finalVerdict": "Would not start"}, "medalRating": "borked"}}
        ]
    });
    let page = parse_page(&response, 1).unwrap();
    assert_eq!(page.next, Some(2));
    let modern = &page.reports[0];
    assert_eq!(modern.device, Device::SteamDeck);
    assert_eq!(modern.proton, Proton::Ge);
    assert_eq!(modern.review.hours_at_review, Some(2.));
    assert_eq!(
        modern.review.text,
        "Worked well\n\nLaunch options\n-windowed"
    );
    assert_eq!(modern.review.recommended(), Some(true));
    assert_eq!(page.reports[1].review.text, "Would not start");
    assert_eq!(page.reports[1].review.recommended(), Some(false));
    assert!(parse_page(&response, 2).is_err());
}

fn fetch_page(app_id: u32, page: u32) -> Result<Page, String> {
    let counts: Counts = get_json("https://www.protondb.com/data/counts.json", 16 * 1024)?;
    if counts.timestamp == 0 {
        return Err("ProtonDB returned an invalid report index.".into());
    }
    let key = report_key(u64::from(app_id), &counts, u64::from(page));
    let response = get_json(
        &format!("https://www.protondb.com/data/reports/all-devices/app/{key}.json"),
        2 * 1024 * 1024,
    )?;
    parse_page(&response, page)
}

fn get_json<T: DeserializeOwned>(url: &str, limit: u64) -> Result<T, String> {
    let client = CLIENT.as_ref().map_err(|_| "ProtonDB is unavailable.")?;
    let response = client
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|_| "Could not load ProtonDB reviews. Try again.")?;
    serde_json::from_reader(response.take(limit))
        .map_err(|_| "ProtonDB returned invalid reviews. Try again.".into())
}

fn report_key(app_id: u64, counts: &Counts, page: u64) -> u32 {
    // ProtonDB's static report filenames use the frontend's signed 32-bit string hash.
    let key = format!(
        "p{}p{}*vRT{app_id}p{}undefinedm",
        counts.reports,
        app_id * u64::from(counts.reports % counts.timestamp),
        page * (app_id % u64::from(counts.timestamp)),
    );
    key.bytes()
        .fold(0_i32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(i32::from(byte))
        })
        .unsigned_abs()
}

fn parse_page(response: &Value, page: u32) -> Result<Page, String> {
    let invalid = || "ProtonDB returned invalid reviews. Try again.".to_owned();
    if response["page"].as_u64() != Some(u64::from(page)) {
        return Err(invalid());
    }
    let rows = response["reports"].as_array().ok_or_else(invalid)?;
    let total = response["total"].as_u64().ok_or_else(invalid)?;
    let per_page = response["perPage"]
        .as_u64()
        .filter(|&count| count > 0)
        .ok_or_else(invalid)?;
    Ok(Page {
        reports: rows.iter().filter_map(parse_report).collect(),
        total,
        next: (!rows.is_empty() && u64::from(page).saturating_mul(per_page) < total)
            .then(|| page.checked_add(1))
            .flatten(),
    })
}

fn value_text(value: &Value) -> &str {
    value.as_str().unwrap_or_default().trim()
}

fn parse_report(row: &Value) -> Option<Report> {
    let id = row["id"].as_str().filter(|id| !id.is_empty())?;
    let responses = &row["responses"];
    let mut notes = Vec::new();
    for value in [
        &responses["notes"]["verdict"],
        &responses["notes"]["finalVerdict"],
        &responses["notes"],
        &responses["concludingNotes"],
    ]
    .into_iter()
    .chain(
        responses["notes"]
            .as_object()
            .into_iter()
            .flat_map(|notes| notes.values()),
    ) {
        let note = value_text(value);
        if !note.is_empty() && !notes.contains(&note) {
            notes.push(note);
        }
    }
    let has_notes = !notes.is_empty();
    let launch_options = value_text(&responses["launchOptions"]);
    let has_launch_options = !launch_options.is_empty();
    let mut text = notes.join("\n\n");
    if has_launch_options {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Launch options\n");
        text.push_str(launch_options);
    }
    let rating = match value_text(&responses["verdict"]) {
        "yes" => "Recommended",
        "no" => "Not recommended",
        _ => match value_text(&responses["medalRating"]) {
            "platinum" | "gold" => "Recommended",
            "silver" => "Silver",
            "bronze" => "Bronze",
            "borked" => "Not recommended",
            _ => "",
        },
    };
    if rating.is_empty() && text.is_empty() {
        return None;
    }
    let (device, platform) = match value_text(&row["device"]["hardwareType"]) {
        "pc" => (Device::Pc, "PC"),
        "steamDeck" => (Device::SteamDeck, "Steam Deck"),
        "chromeOs" | "chromeOS" | "chromebook" => (Device::Chromebook, "Chromebook"),
        _ => (Device::Other, ""),
    };
    let variant = value_text(&responses["variant"]);
    let proton = match variant {
        "official" | "older" => Proton::Official,
        "experimental" => Proton::Experimental,
        "ge" => Proton::Ge,
        "native" => Proton::Native,
        _ => Proton::Other,
    };
    let custom = value_text(&responses["customProtonVersion"]);
    let version = value_text(&responses["protonVersion"]);
    let runtime = match (proton, custom.is_empty(), version.is_empty()) {
        (Proton::Native, _, _) => "Native Linux".into(),
        (Proton::Experimental, _, _) => "Proton Experimental".into(),
        (_, false, _) => custom.to_owned(),
        (_, _, false) => format!("Proton {version}"),
        _ => String::new(),
    };
    let steam = &row["contributor"]["steam"];
    Some(Report {
        review: Review {
            id: id.to_owned(),
            author: value_text(&steam["nickname"]).to_owned(),
            text,
            rating: rating.to_owned(),
            date: row["timestamp"]
                .as_i64()
                .map(|time| details::display_date(&time.to_string()))
                .unwrap_or_default(),
            hours_at_review: steam["playtime"].as_f64().map(|minutes| minutes / 60.),
            platform: [platform, runtime.as_str()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            ..Default::default()
        },
        device,
        proton,
        has_notes,
        has_launch_options,
    })
}

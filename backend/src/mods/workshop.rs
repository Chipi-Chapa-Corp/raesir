use super::*;
use scraper::{Html, Selector};
use serde_json::Value;
use std::io::Read;

const SOURCE: &str = "Steam Workshop";

impl Manager {
    pub fn workshop_page(
        &self,
        game: &LibraryGame,
        query: &str,
        page: u32,
        order: CatalogOrder,
        refresh: bool,
    ) -> Result<CatalogPage, String> {
        let Some(app) = game
            .id
            .parse::<u64>()
            .ok()
            .filter(|_| game.store == "steam")
        else {
            return Ok(CatalogPage::default());
        };
        let sort = match order {
            CatalogOrder::Downloads => "totaluniquesubscribers",
            CatalogOrder::Trending => "trend",
            CatalogOrder::All if !query.is_empty() => "trend",
            CatalogOrder::All => "mostrecent",
        };
        let url = format!(
            "https://steamcommunity.com/workshop/browse/?appid={app}&section=readytouseitems&browsesort={sort}&p={page}&searchtext={}&l=english",
            crate::catalog::encode(query)
        );
        let fetch = || {
            if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
                return Err("Mod networking is disabled".into());
            }
            let http = providers::http()?;
            retry_catalog(
                || {
                    let response = http
                        .get(&url)
                        .timeout(Duration::from_secs(10))
                        .send()
                        .map_err(|error| error.to_string())?;
                    if response.status() == reqwest::StatusCode::FORBIDDEN {
                        return Err("Steam denied access to the Workshop catalog (403).".into());
                    }
                    let response = response
                        .error_for_status()
                        .map_err(|error| error.to_string())?;
                    // Steam sends games without a Workshop to the global Workshop page.
                    if response.url().path().trim_end_matches('/') != "/workshop/browse"
                        || !response
                            .url()
                            .query_pairs()
                            .any(|(key, value)| key == "appid" && value == app.to_string())
                    {
                        return Ok(Value::Null);
                    }
                    const LIMIT: u64 = 8 * 1024 * 1024;
                    let mut body = String::new();
                    response
                        .take(LIMIT + 1)
                        .read_to_string(&mut body)
                        .map_err(|error| error.to_string())?;
                    if body.len() as u64 > LIMIT {
                        return Err("Steam Workshop response is too large".into());
                    }
                    workshop_results(&body, page)
                },
                std::thread::sleep,
            )
        };
        let data = if query.is_empty() && page == 1 {
            self.provider_json_inner(&url, refresh, true, fetch)?
        } else {
            fetch()?
        };
        if data.is_null() {
            return Ok(CatalogPage::default());
        }
        let packages = data["results"]
            .as_array()
            .ok_or("Invalid Steam Workshop results")?
            .iter()
            .filter_map(|row| package(row, app))
            .collect();
        let next = data["total_pages"]
            .as_u64()
            .filter(|&total| total > u64::from(page))
            .map(|_| page + 1);
        Ok(CatalogPage {
            packages,
            next,
            ..Default::default()
        })
    }
}

fn retry_catalog(
    mut fetch: impl FnMut() -> Result<Value, String>,
    mut wait: impl FnMut(Duration),
) -> Result<Value, String> {
    for delay in [1, 2, 3] {
        match fetch() {
            Ok(page) => return Ok(page),
            Err(_) => wait(Duration::from_secs(delay)),
        }
    }
    fetch()
}

fn workshop_results(html: &str, page: u32) -> Result<Value, String> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("script#valve-ssr-data").expect("valid Steam selector");
    let Some(script) = doc.select(&selector).next() else {
        return Err("Steam Workshop did not return a catalog".into());
    };
    let ssr: Value =
        serde_json::from_str(&script.inner_html()).map_err(|error| error.to_string())?;
    let query_data = ssr["renderContext"]["queryData"]
        .as_str()
        .ok_or("Missing Workshop page data")?;
    let queries: Value = serde_json::from_str(query_data).map_err(|error| error.to_string())?;
    queries["queries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|query| &query["state"]["data"])
        .find(|data| data["current_page"] == page && data["results"].is_array())
        .cloned()
        .ok_or_else(|| "Steam Workshop did not return this page of results".into())
}

fn package(row: &Value, app: u64) -> Option<Package> {
    if row["consumer_appid"].as_u64() != Some(app) {
        return None;
    }
    let id = row["publishedfileid"].as_str()?.parse::<u64>().ok()?;
    let icon = row["preview_url"]
        .as_str()
        .and_then(|image| reqwest::Url::parse(image).ok())
        .filter(|url| {
            url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some("images.steamusercontent.com" | "steamuserimages-a.akamaihd.net")
                )
        })
        .map_or_else(String::new, |url| url.to_string());
    Some(Package {
        id: format!("workshop:{id}"),
        name: row["title"].as_str()?.into(),
        owner: String::new(),
        description: row["short_description"].as_str().unwrap_or_default().into(),
        version: String::new(),
        page: format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}"),
        icon,
        downloads: row["subscriptions"].as_u64().unwrap_or_default(),
        deprecated: false,
        categories: Vec::new(),
        source: SOURCE.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_retries_wait_between_failures_and_stop_on_success() {
        for (succeed_at, expected_delays) in [(1, &[][..]), (3, &[1, 2][..]), (5, &[1, 2, 3][..])] {
            let mut attempts = 0;
            let mut delays = Vec::new();
            let result = retry_catalog(
                || {
                    attempts += 1;
                    if attempts == succeed_at {
                        Ok(json!("catalog"))
                    } else {
                        Err(format!("attempt {attempts} failed"))
                    }
                },
                |delay| delays.push(delay.as_secs()),
            );
            assert_eq!(delays, expected_delays);
            assert_eq!(attempts, expected_delays.len() + 1);
            if succeed_at == 5 {
                assert_eq!(result.unwrap_err(), "attempt 4 failed");
            } else {
                assert_eq!(result.unwrap(), json!("catalog"));
            }
        }
    }

    #[test]
    fn decodes_public_workshop_page_and_ignores_other_apps() {
        let queries = json!({
            "queries": [{"state": {"data": {
                "current_page": 2, "total_pages": 3,
                "results": [
                    {"consumer_appid": 730, "publishedfileid": "90", "title": "Test map"},
                    {"consumer_appid": 570, "publishedfileid": "91", "title": "Other game"}
                ]
            }}}]
        });
        let ssr = json!({"renderContext": {"queryData": queries.to_string()}});
        let html = format!(r#"<script type="application/json" id="valve-ssr-data">{ssr}</script>"#);
        let page = workshop_results(&html, 2).unwrap();
        let packages = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| package(row, 730))
            .collect::<Vec<_>>();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].page,
            "https://steamcommunity.com/sharedfiles/filedetails/?id=90"
        );
        assert!(workshop_results(&html, 1).is_err());
    }

    #[test]
    fn reads_public_workshop_details_and_preserves_original_images() {
        let html = r#"
            <meta property="og:image" content="https://images.steamusercontent.com/example?crop=1">
            <a data-appid="730" href="https://store.steampowered.com/app/730/"></a>
            <div class="workshopItemTitle">Test map</div>
            <div class="workshopItemDescription" id="highlightContent">Map <b>description</b></div>
            <div class="creatorsBlock"><div class="friendBlockContent">Creator<br>Offline</div></div>
            <div class="detailsStatLeft">Posted</div><div class="detailsStatRight">Aug 12</div>
            <div class="detailsStatLeft">Updated</div><div class="detailsStatRight">Aug 13</div>
            <table class="stats_table">
                <tr><td>1,234</td><td>Unique Visitors</td></tr>
                <tr><td>42</td><td>Current Subscribers</td></tr>
            </table>
            <div class="highlight_strip_screenshot"><img src="https://images.steamusercontent.com/example?w=64"></div>
            <div class="commentthread_comment">
                <a class="commentthread_author_link">A player</a>
                <div class="commentthread_comment_text">Nice map!</div>
            </div>"#;
        let page = "https://steamcommunity.com/sharedfiles/filedetails/?id=90";
        assert!(workshop_detail_page(html, page, 570, 90).is_err());
        let details = workshop_detail_page(html, page, 730, 90).unwrap();
        assert_eq!(details.description, "Map description");
        assert_eq!(details.developers, ["Creator"]);
        assert_eq!(details.media.len(), 1);
        assert_eq!(
            details.media[0].url,
            "https://images.steamusercontent.com/example"
        );
        let metadata = details.mod_metadata.unwrap();
        assert_eq!(metadata.last_updated, "Aug 13");
        assert_eq!(metadata.views, Some(1234));
        assert_eq!(metadata.downloads, None);
        assert_eq!(metadata.comments[0].author, "A player");
        assert_eq!(metadata.comments[0].text, "Nice map!");

        let changelog = workshop_changelog(
            r#"<div class="changeLogCtn"><div class="changelog author"><a>Creator</a></div>
            <p id="1711026000">Fixed a wall &amp; a door.</p></div>"#,
        );
        assert_eq!(changelog[0].text, "Fixed a wall & a door.");
        assert_eq!(changelog[0].author, "Creator");
        assert!(!changelog[0].date.is_empty());
    }
}

impl Manager {
    pub(super) fn workshop_details(
        &self,
        game: &LibraryGame,
        id: &str,
    ) -> Result<crate::details::Details, String> {
        let id = id
            .strip_prefix("workshop:")
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|_| game.store == "steam")
            .ok_or("Invalid Workshop item")?;
        let app = game.id.parse::<u64>().map_err(|_| "Invalid Steam app ID")?;
        let page = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}");
        let url = format!("{page}&l=english");
        let html = self.provider_json_inner(&url, false, true, || {
            providers::html(&url).map(Value::String)
        })?;
        let html = html.as_str().ok_or("Invalid Workshop item page")?;
        let mut details = workshop_detail_page(html, &page, app, id)?;
        if html.contains("detailsStatNumChangeNotes") {
            let url = format!(
                "https://steamcommunity.com/sharedfiles/filedetails/changelog/{id}?l=english"
            );
            let logs = self.provider_json_inner(&url, false, true, || {
                providers::html(&url).map(Value::String)
            });
            if let Some(html) = logs.as_ref().ok().and_then(Value::as_str) {
                details.mod_metadata.as_mut().unwrap().changelog = workshop_changelog(html);
            }
        }
        Ok(details)
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("valid Workshop selector")
}

fn element_text(value: scraper::ElementRef<'_>) -> String {
    value
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn item_image(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("images.steamusercontent.com" | "steamuserimages-a.akamaihd.net")
        )
    {
        return None;
    }
    url.set_query(None);
    Some(url.to_string())
}

fn workshop_detail_page(
    html: &str,
    page: &str,
    app: u64,
    id: u64,
) -> Result<crate::details::Details, String> {
    use crate::details::{Details, Media, ModMetadata, ModPost, display_date, rich};
    let doc = Html::parse_document(html);
    if doc
        .select(&selector(
            "a[data-appid][href*='store.steampowered.com/app/']",
        ))
        .next()
        .and_then(|node| node.value().attr("data-appid"))
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value != app)
    {
        return Err("This Workshop item belongs to another Steam game".into());
    }
    let select = |css| {
        doc.select(&selector(css))
            .next()
            .map(element_text)
            .unwrap_or_default()
    };
    let title = select(".workshopItemTitle");
    if title.is_empty() {
        return Err("Steam did not return a public Workshop item".into());
    }
    let markup = doc
        .select(&selector("#highlightContent.workshopItemDescription"))
        .next()
        .map(|node| node.inner_html())
        .unwrap_or_default();
    let author = select(".creatorsBlock .friendBlockContent");
    let author = doc
        .select(&selector(".creatorsBlock .friendBlockContent"))
        .next()
        .and_then(|node| node.text().map(str::trim).find(|text| !text.is_empty()))
        .map(str::to_owned)
        .unwrap_or(author);
    let keys = selector(".detailsStatLeft");
    let values = selector(".detailsStatRight");
    let dates = doc
        .select(&keys)
        .map(element_text)
        .zip(doc.select(&values).map(element_text))
        .collect::<HashMap<_, _>>();
    let views = doc.select(&selector(".stats_table tr")).find_map(|row| {
        let cells = selector("td");
        let mut cells = row.select(&cells).map(element_text);
        let count = cells.next()?;
        (cells.next()?.eq_ignore_ascii_case("unique visitors"))
            .then(|| count.replace(',', "").parse::<u64>().ok())
            .flatten()
    });
    let screenshots = selector(".highlight_strip_screenshot img, meta[property='og:image']");
    let mut visible = std::collections::HashSet::new();
    let media = doc
        .select(&screenshots)
        .filter_map(|node| {
            let original = node
                .value()
                .attr("src")
                .or_else(|| node.value().attr("content"))?;
            let url = item_image(original)?;
            visible.insert(url.clone()).then(|| Media {
                url,
                preview: String::new(),
                title: String::new(),
                video: false,
            })
        })
        .take(32)
        .collect();
    let comments = doc
        .select(&selector(".commentthread_comment"))
        .filter_map(|node| {
            let text = node
                .select(&selector(".commentthread_comment_text"))
                .next()
                .map(|body| providers::plain_text(&body.inner_html()))?;
            if text.is_empty() {
                return None;
            }
            let author = node
                .select(&selector(".commentthread_author_link"))
                .next()
                .map(element_text)
                .unwrap_or_default();
            let date = node
                .select(&selector(
                    "[data-timestamp].commentthread_comment_timestamp",
                ))
                .next()
                .and_then(|date| date.value().attr("data-timestamp"))
                .map(display_date)
                .unwrap_or_default();
            Some(ModPost {
                author,
                date,
                text,
                ..Default::default()
            })
        })
        .take(10)
        .collect();
    Ok(Details {
        key: format!("workshop:{id}"),
        title,
        store_url: Some(page.into()),
        description: providers::plain_text(&markup).chars().take(600).collect(),
        description_blocks: rich::parse(&markup, page),
        developers: (!author.is_empty()).then_some(author).into_iter().collect(),
        released: dates.get("Posted").cloned().unwrap_or_default(),
        media,
        mod_metadata: Some(ModMetadata {
            last_updated: dates.get("Updated").cloned().unwrap_or_default(),
            views,
            comments,
            ..Default::default()
        }),
        sources: vec![SOURCE.into()],
        ..Default::default()
    })
}

fn workshop_changelog(html: &str) -> Vec<crate::details::ModPost> {
    let doc = Html::parse_document(html);
    doc.select(&selector(".changeLogCtn"))
        .filter_map(|node| {
            let body = node.select(&selector("p[id]")).next()?;
            let date = body
                .value()
                .attr("id")
                .map(crate::details::display_date)
                .unwrap_or_default();
            let text = providers::plain_text(&body.inner_html());
            let author = node
                .select(&selector(".changelog.author a"))
                .next()
                .map(element_text)
                .unwrap_or_default();
            Some(crate::details::ModPost {
                date,
                text,
                author,
                ..Default::default()
            })
        })
        .take(10)
        .collect()
}

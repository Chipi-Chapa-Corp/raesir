//! Public game pages and thumbnails; no account or API credentials.
use super::*;

pub(super) fn portrait(
    appid: u32,
    title: &str,
    page: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let search = page(&format!(
        "https://gamesdb.launchbox-app.com/games/results/{}%7C1",
        catalog::encode(title)
    ))?;
    for path in candidates(&search, appid, title) {
        let html = page(&format!("https://gamesdb.launchbox-app.com{path}"))?;
        if let Some(portrait) = select(&html, appid) {
            return Ok(Some(portrait));
        }
    }
    Ok(None)
}

fn steam_link(value: &str, appid: u32) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|url| {
        url.host_str() == Some("store.steampowered.com")
            && url.path().trim_end_matches('/') == format!("/app/{appid}")
    })
}

fn candidates(html: &str, appid: u32, title: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let links = Selector::parse("#cardsContainer a[href^='/games/details/']").unwrap();
    let icons = Selector::parse("img[onclick]").unwrap();
    let heading = Selector::parse("h3").unwrap();
    let mut matches = Vec::new();
    for link in doc.select(&links) {
        let exact_id = link.select(&icons).any(|img| {
            img.value().attr("onclick").is_some_and(|script| {
                // Read the public Steam link as text; never execute page scripts.
                script.split(['\'', '"']).any(|s| steam_link(s, appid))
            })
        });
        let exact_title = link.select(&heading).next().is_some_and(|h| {
            super::super::normalize(&h.text().collect::<String>()) == super::super::normalize(title)
        });
        if exact_id || exact_title {
            matches.push((!exact_id, link.value().attr("href").unwrap().to_owned()));
        }
    }
    matches.sort();
    matches.dedup();
    // Every candidate must still prove its Steam ID on the detail page.
    matches.into_iter().take(3).map(|(_, path)| path).collect()
}

fn select(html: &str, appid: u32) -> Option<String> {
    let doc = Html::parse_document(html);
    let links = Selector::parse("a[href]").ok()?;
    if !doc.select(&links).any(|a| {
        a.value()
            .attr("href")
            .is_some_and(|href| steam_link(href, appid))
    }) {
        return None;
    }
    let selector = Selector::parse("img[alt][src]").ok()?;
    doc.select(&selector)
        .filter_map(|img| {
            let alt = img.value().attr("alt")?;
            alt.split_once(" - Box - Front (")?;
            let dimensions = alt.rsplit_once(" - ")?.1;
            let (width, height) = dimensions.split_once('x')?;
            let width = width.trim().parse::<u32>().ok()?;
            let height = height.trim().parse::<u32>().ok()?;
            if !(200..=10_000).contains(&width) || height <= width || height > width * 2 {
                return None;
            }
            let url = img.value().attr("src")?;
            let parsed = reqwest::Url::parse(url).ok()?;
            if parsed.scheme() != "https"
                || !matches!(
                    parsed.host_str(),
                    Some("images.launchbox-app.com" | "gamesdb-images.launchbox.gg")
                )
            {
                return None;
            }
            // Prefer a designed 2:3 front cover, using the page's thumbnail URL.
            let ratio_error = (width * 3).abs_diff(height * 2) * 1000 / height;
            Some((ratio_error, url.to_owned()))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, url)| url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_search_matches_steam_id_and_bounds_title_candidates() {
        let html = r#"<div id="cardsContainer">
            <a href="/games/details/1-wrong"><h3>ARK: Survival of the Fittest</h3></a>
            <a href="/games/details/49766-ark-survival-of-the-fittest"><h3>ARK: The Survival of the Fittest</h3><img onclick="window.open('https://store.steampowered.com/app/407530', '_blank'); return false;"></a>
            <a href="https://untrusted.example/games/details/1"><h3>ARK: Survival of the Fittest</h3></a>
            </div>"#;
        let results = candidates(html, 407530, "ARK: Survival Of The Fittest");
        assert_eq!(
            results[0],
            "/games/details/49766-ark-survival-of-the-fittest"
        );
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn portrait_requires_exact_steam_id_and_rejects_banners_and_3d_boxes() {
        let html = r#"<a href="https://store.steampowered.com/app/407530">Steam</a>
            <img alt="ARK - Banner (World) - 600x900" src="https://images.launchbox-app.com/banner.jpg">
            <img alt="ARK - Box - 3D (World) - 600x900" src="https://images.launchbox-app.com/3d.jpg">
            <img alt="ARK - Box - Front (World) - 647x920" src="https://images.launchbox-app.com/world.jpg">
            <img alt="ARK - Box - Front (North America) - 600x900" src="https://images.launchbox-app.com/portrait.png">
            <img alt="ARK - Box - Front (World) - 600x900" src="https://untrusted.example/image.png">"#;
        assert_eq!(
            select(html, 407530).as_deref(),
            Some("https://images.launchbox-app.com/portrait.png")
        );
        assert!(select(html, 346110).is_none());
        let wide = r#"<a href="https://store.steampowered.com/app/407530">Steam</a><img alt="ARK - Box - Front (World) - 616x353" src="https://images.launchbox-app.com/banner.jpg">"#;
        assert!(select(wide, 407530).is_none());
        let migrated = html.replace("images.launchbox-app.com", "gamesdb-images.launchbox.gg");
        assert_eq!(
            select(&migrated, 407530).as_deref(),
            Some("https://gamesdb-images.launchbox.gg/portrait.png")
        );
    }
}

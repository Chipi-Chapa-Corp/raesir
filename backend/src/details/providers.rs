use super::*;

pub(super) fn steam(
    client: &Client,
    id: &str,
    out: &mut Details,
    with_reviews: bool,
) -> Result<(), String> {
    if !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Steam product ID is missing".into());
    }
    let value = get_json(
        client,
        &format!("https://store.steampowered.com/api/appdetails?appids={id}&cc=us&l=english"),
    )?;
    let data = &value[id]["data"];
    if !value[id]["success"].as_bool().unwrap_or(false) {
        return Err("This Steam edition has no public store page".into());
    }
    parse_steam(data, out);
    out.store_url = Some(format!("https://store.steampowered.com/app/{id}/"));
    out.sources.push("Steam".into());
    if with_reviews {
        let input = json!({"appid":id.parse::<u64>().unwrap_or(0),"filter":1,"languages":["english"],"review_type":0,"purchase_type":0,"num_per_page":12});
        match get_json(
            client,
            &format!(
                "https://api.steampowered.com/IUserReviewsService/GetAppReviews/v1/?input_json={}",
                crate::catalog::encode(&input.to_string())
            ),
        ) {
            Ok(value) => parse_steam_reviews(value.get("response").unwrap_or(&value), out),
            Err(_) => out
                .warnings
                .push("Steam reviews could not be loaded. Try refreshing this page.".into()),
        }
    }
    Ok(())
}
fn parse_steam(data: &Value, out: &mut Details) {
    out.set_description(text(data, "detailed_description"));
    if out.description.is_empty() {
        out.set_description(text(data, "short_description"));
    }
    out.developers = strings(data, "developers");
    out.publishers = strings(data, "publishers");
    out.released = text(&data["release_date"], "date");
    out.genres = array(data, "genres")
        .iter()
        .map(|g| text(g, "description"))
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(score) = data["metacritic"]["score"].as_u64() {
        out.scores.push(format!("Metacritic {score}/100"));
    }
    for rating in ["esrb", "pegi", "usk"] {
        let r = text(&data["ratings"][rating], "rating");
        if !r.is_empty() {
            out.age_rating = format!("{} {}", rating.to_uppercase(), r);
            break;
        }
    }
    if out.age_rating.is_empty() {
        if let Some(age) = data["required_age"].as_u64().filter(|n| *n > 0) {
            out.age_rating = format!("{age}+");
        }
    }
    for movie in array(data, "movies") {
        let url = [
            text(movie, "hls_h264"),
            text(movie, "dash_h264"),
            text(&movie["mp4"], "480"),
            text(&movie["webm"], "480"),
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
        if !url.is_empty() {
            out.media.push(Media {
                url,
                preview: text(movie, "thumbnail"),
                title: text(movie, "name"),
                video: true,
            });
        }
    }
    for image in array(data, "screenshots").iter().take(16) {
        out.media.push(Media {
            url: text(image, "path_full"),
            preview: text(image, "path_thumbnail"),
            title: "Screenshot".into(),
            video: false,
        });
    }
    for (platform, key) in [
        ("Windows", "pc_requirements"),
        ("Linux", "linux_requirements"),
        ("macOS", "mac_requirements"),
    ] {
        let minimum = requirement_text(&text(&data[key], "minimum"));
        let recommended = requirement_text(&text(&data[key], "recommended"));
        if !minimum.is_empty() || !recommended.is_empty() {
            out.requirements.push(Requirement {
                platform: platform.into(),
                minimum,
                recommended,
            });
        }
    }
}
fn requirement_text(s: &str) -> String {
    super::requirement_text(&clean(s))
        .lines()
        .filter(|line| {
            !matches!(
                line.trim_end_matches(':').to_ascii_lowercase().as_str(),
                "minimum" | "recommended"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_steam_reviews(value: &Value, out: &mut Details) {
    let summary = &value["query_summary"];
    let label = text(summary, "review_score_desc");
    if let Some(total) = summary["total_reviews"].as_u64() {
        out.review_summary = format!("{label} · {total} reviews");
        if let Some(positive) = summary["total_positive"].as_u64().filter(|_| total > 0) {
            out.scores.insert(
                0,
                format!("Steam players {:.1}/5", positive as f64 / total as f64 * 5.),
            );
        }
    }
    out.review_source = "Steam players".into();
    out.reviews = array(value, "reviews")
        .iter()
        .map(|r| Review {
            author: match text(&r["author"], "personaname") {
                name if !name.is_empty() => name,
                _ => "Steam player".into(),
            },
            title: String::new(),
            text: strip_bbcode(&text(r, "review")),
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
            platform: String::new(),
        })
        .filter(|r| !r.text.is_empty())
        .collect();
}
fn strip_bbcode(input: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in input.chars() {
        if c == '[' {
            tag = true;
        } else if c == ']' {
            tag = false;
        } else if !tag {
            out.push(c);
        }
    }
    out
}

pub(super) fn xbox(client: &Client, game: &LibraryGame, out: &mut Details) -> Result<(), String> {
    if game.id.len() != 12 {
        return Err("This Xbox entry has no Microsoft Store product ID".into());
    }
    let value = get_json(
        client,
        &format!(
            "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds={}&market=US&languages=en-us",
            crate::catalog::encode(&game.id)
        ),
    )?;
    let product = array(&value, "Products")
        .first()
        .ok_or("Microsoft Store product was not found")?;
    parse_xbox(product, out);
    if out.requirements.is_empty() {
        let mut ids = Vec::new();
        collect_product_ids(&product["Properties"]["BundledSkus"], &mut ids);
        ids.sort();
        ids.dedup();
        for id in ids.into_iter().filter(|id| id != &game.id).take(4) {
            if let Ok(v) = get_json(
                client,
                &format!(
                    "https://displaycatalog.mp.microsoft.com/v7.0/products/{id}?market=US&languages=en-us"
                ),
            ) {
                let child = v
                    .get("Product")
                    .or_else(|| array(&v, "Products").first())
                    .unwrap_or(&v);
                let mut candidate = Details::default();
                parse_xbox(child, &mut candidate);
                if !candidate.requirements.is_empty() {
                    out.requirements = candidate.requirements;
                    break;
                }
            }
        }
    }
    let url = format!(
        "https://emerald.xboxservices.com/xboxcomfd/ratingsandreviews/summaryandreviews/{}?locale=en-US&orderBy=MostHelpful&itemCount=12&starFilter=NoFilter",
        crate::catalog::encode(&game.id)
    );
    let result = response(
        client
            .get(&url)
            .header("x-ms-api-version", "1.0")
            .header("MS-CV", "AAAAAAAAAAAAAAAAAAAAAA.0"),
    )
    .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string()));
    match result {
        Ok(v) => parse_xbox_reviews(&v, out),
        Err(_) => out
            .warnings
            .push("Microsoft Store reviews could not be loaded.".into()),
    }
    out.sources.push("Microsoft Store".into());
    Ok(())
}
fn parse_xbox(product: &Value, out: &mut Details) {
    if let Some(local) = array(product, "LocalizedProperties").first() {
        out.set_description(text(local, "ProductDescription"));
        if out.description.is_empty() {
            out.set_description(text(local, "ShortDescription"));
        }
        out.developers = vec![text(local, "DeveloperName")];
        out.publishers = vec![text(local, "PublisherName")];
        for v in array(local, "CMSVideos") {
            let url = text(v, "HLS");
            if !url.is_empty() {
                out.media.push(Media {
                    url,
                    preview: absolute(
                        &text(&v["PreviewImage"], "Uri"),
                        "https://www.microsoft.com",
                    ),
                    title: text(v, "Caption"),
                    video: true,
                });
            }
        }
        for v in array(local, "Images")
            .iter()
            .filter(|v| text(v, "ImagePurpose") == "Screenshot")
            .take(16)
        {
            out.media.push(Media {
                url: absolute(&text(v, "Uri"), "https://www.microsoft.com"),
                preview: String::new(),
                title: text(v, "Caption"),
                video: false,
            });
        }
    }
    let properties = &product["Properties"];
    out.genres = strings(properties, "Categories");
    for market in array(product, "MarketProperties") {
        if out.released.is_empty() {
            out.released = text(market, "OriginalReleaseDate")
                .chars()
                .take(10)
                .collect();
        }
        if out.age_rating.is_empty() {
            let ratings = array(market, "ContentRatings");
            if let Some(r) = ratings
                .iter()
                .find(|r| text(r, "RatingSystem") == "ESRB")
                .or_else(|| ratings.first())
            {
                out.age_rating = text(r, "RatingId").replace(':', " ");
            }
        }
        if let Some(r) = array(market, "UsageData")
            .iter()
            .find(|r| r["AverageRating"].as_f64().is_some_and(|n| n > 0.))
        {
            out.scores.push(format!(
                "Microsoft Store {:.1}/5",
                r["AverageRating"].as_f64().unwrap_or(0.)
            ));
            break;
        }
    }
    for entry in array(product, "DisplaySkuAvailabilities") {
        let h = &entry["Sku"]["Properties"]["HardwareProperties"];
        let mut minimum = Vec::new();
        let mut recommended = Vec::new();
        for (label, key) in [("Processor", "Processor"), ("Graphics", "Graphics")] {
            let a = text(h, &format!("Minimum{key}"));
            if !a.is_empty() {
                minimum.push(format!("{label}: {a}"));
            }
            let b = text(h, &format!("Recommended{key}"));
            if !b.is_empty() {
                recommended.push(format!("{label}: {b}"));
            }
        }
        for (key, target) in [
            ("MinimumHardware", &mut minimum),
            ("RecommendedHardware", &mut recommended),
        ] {
            for code in strings(h, key) {
                if let Some(name) = hardware(&code) {
                    target.push(name);
                }
            }
        }
        if !minimum.is_empty() || !recommended.is_empty() {
            out.requirements.push(Requirement {
                platform: "Windows".into(),
                minimum: minimum.join("\n"),
                recommended: recommended.join("\n"),
            });
            break;
        }
    }
}
fn hardware(code: &str) -> Option<String> {
    match code {
        "kbd" => Some("Keyboard".into()),
        "mse" => Some("Mouse".into()),
        "xgp" => Some("Xbox controller".into()),
        _ => code
            .strip_prefix('m')
            .and_then(|v| v.parse::<u32>().ok())
            .map(|n| format!("Memory: {n} GB")),
    }
}
fn collect_product_ids(v: &Value, ids: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            if let Some(s) = o.get("ProductId").and_then(Value::as_str) {
                ids.push(s.into());
            }
            for v in o.values() {
                collect_product_ids(v, ids);
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_product_ids(v, ids);
            }
        }
        _ => {}
    }
}
fn parse_xbox_reviews(value: &Value, out: &mut Details) {
    out.review_source = "Microsoft Store customers".into();
    let summary = &value["ratingsSummary"];
    let count = summary
        .get("totalRatingsCount")
        .or_else(|| summary.get("ratingsCount"))
        .and_then(Value::as_u64)
        .or_else(|| value["totalReviews"].as_u64());
    if let Some(count) = count {
        out.review_summary = format!("{count} customer reviews");
    }
    out.reviews = array(value, "reviews")
        .iter()
        .map(|r| Review {
            author: text(r, "userName"),
            title: text(r, "title"),
            text: text(r, "reviewText"),
            rating: r["rating"]
                .as_f64()
                .map(|n| format!("{n}/5"))
                .unwrap_or_default(),
            date: display_date(&text(r, "submittedDateTime")),
            hours: None,
            platform: review_platform(&text(r, "deviceFamily")),
        })
        .filter(|r| !r.text.is_empty())
        .collect();
}

fn epic_product(client: &Client, game: &LibraryGame) -> Result<(Value, String, String), String> {
    let page = store_url(game)
        .and_then(|u| u.split("/p/").nth(1))
        .map(|s| s.split('?').next().unwrap_or(s).to_owned());
    let mut namespace = String::new();
    let slug = if let Some(page) = page {
        page
    } else {
        let query = format!(
            "query {{ Catalog {{ searchStore(keywords:{},country:\"US\",locale:\"en-US\",count:20) {{ elements {{ title id namespace productSlug catalogNs {{ mappings {{ pageSlug pageType }} }} }} }} }} }}",
            json!(game.title)
        );
        let value = post_json(
            client,
            "https://store.epicgames.com/graphql",
            json!({"query":query}),
        )?;
        let offer = array(&value["data"]["Catalog"]["searchStore"], "elements")
            .iter()
            .find(|v| text(v, "id") == game.id || same_title(&text(v, "title"), &game.title))
            .ok_or("This Epic edition could not be matched to a public product page")?;
        namespace = text(offer, "namespace");
        array(&offer["catalogNs"], "mappings")
            .iter()
            .find(|v| text(v, "pageType") == "productHome")
            .map(|v| text(v, "pageSlug"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| text(offer, "productSlug"))
    };
    if slug.is_empty() {
        return Err("Epic product page ID is missing".into());
    }
    let value = get_json(
        client,
        &format!(
            "https://store-content.ak.epicgames.com/api/en-US/content/products/{}",
            crate::catalog::encode(&slug)
        ),
    )?;
    let data = array(&value, "pages")
        .iter()
        .find(|p| text(p, "type") == "productHome")
        .map(|p| &p["data"])
        .ok_or("Epic returned no product information")?;
    Ok((
        data.clone(),
        namespace,
        format!("https://store.epicgames.com/p/{slug}"),
    ))
}

fn epic_portrait(data: &Value) -> Option<String> {
    let url = text(&data["about"]["image"], "src");
    safe_url(&url).then_some(url)
}

pub(crate) fn epic_cover(client: &Client, game: &LibraryGame) -> Result<String, String> {
    let (data, _, _) = epic_product(client, game)?;
    epic_portrait(&data).ok_or("Epic returned no portrait artwork".into())
}

pub(super) fn epic(client: &Client, game: &LibraryGame, out: &mut Details) -> Result<(), String> {
    let (data, namespace, url) = epic_product(client, game)?;
    out.store_url = Some(url);
    let data = &data;
    parse_epic(data, out);
    for item in array(&data["carousel"], "items").iter().take(8) {
        let video = &item["video"];
        if let Ok(recipes) = serde_json::from_str::<Value>(&text(video, "recipes")) {
            let recipes = array(&recipes, "en-US");
            let recipe = recipes
                .iter()
                .find(|r| text(r, "recipe") == "video-hls")
                .or_else(|| recipes.first());
            if let Some(recipe) = recipe {
                let id = text(recipe, "mediaRefId");
                let query = format!(
                    "query {{ Media {{ getMediaRef(mediaRefId:{}) {{ outputs {{ url }} }} }} }}",
                    json!(id)
                );
                if let Ok(value) = post_json(
                    client,
                    "https://store.epicgames.com/graphql",
                    json!({"query":query}),
                ) {
                    if let Some(url) = array(&value["data"]["Media"]["getMediaRef"], "outputs")
                        .iter()
                        .map(|v| text(v, "url"))
                        .find(|s| safe_url(s))
                    {
                        out.media.insert(
                            0,
                            Media {
                                url,
                                preview: text(&item["image"], "src"),
                                title: text(video, "title"),
                                video: true,
                            },
                        );
                    }
                }
            }
        }
    }
    if !namespace.is_empty() {
        let query = format!(
            "query {{ RatingsPolls {{ getProductResult(sandboxId:{},locale:\"en-US\") {{ averageRating }} }} }}",
            json!(namespace)
        );
        if let Ok(v) = post_json(
            client,
            "https://store.epicgames.com/graphql",
            json!({"query":query}),
        ) {
            if let Some(score) =
                v["data"]["RatingsPolls"]["getProductResult"]["averageRating"].as_f64()
            {
                out.scores.push(format!("Epic players {score:.1}/5"));
            }
        }
    }
    out.sources.push("Epic Games Store".into());
    Ok(())
}
fn parse_epic(data: &Value, out: &mut Details) {
    let about = &data["about"];
    let meta = &data["meta"];
    let markdown = text(about, "description");
    let mut html = String::new();
    pulldown_cmark::html::push_html(
        &mut html,
        pulldown_cmark::Parser::new_ext(&markdown, pulldown_cmark::Options::ENABLE_STRIKETHROUGH),
    );
    out.set_description(html);
    out.developers = vec![text(about, "developerAttribution")];
    out.publishers = vec![text(about, "publisherAttribution")];
    out.released = text(meta, "releaseDate");
    for item in array(&data["carousel"], "items") {
        let url = text(&item["image"], "src");
        if !url.is_empty() {
            out.media.push(Media {
                preview: url.clone(),
                url,
                title: "Screenshot".into(),
                video: false,
            });
        }
    }
    for system in array(&data["requirements"], "systems") {
        let fields = array(system, "details");
        let render = |key: &str| {
            fields
                .iter()
                .filter_map(|f| {
                    let value = text(f, key);
                    (!value.trim().is_empty())
                        .then(|| format!("{}: {}", text(f, "title").trim(), value.trim()))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        out.requirements.push(Requirement {
            platform: text(system, "systemType"),
            minimum: render("minimum"),
            recommended: render("recommended"),
        });
    }
    out.age_rating = text(&data["requirements"]["rating"], "rating");
}

fn ea_product(client: &Client, game: &LibraryGame) -> Result<Value, String> {
    let body = json!({"query":game.title,"context":{"address_country":"US","language":"en-US","currency":"USD"},"pagination":{"limit":20}});
    let bytes = response(
        client
            .post("https://store.ea.com/commerce/api/catalog/search")
            .header("request-id", format!("raesir-details-{}", now()))
            .header("ucp-agent", "Raesir/0.1")
            .json(&body),
    )?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "EA returned invalid catalog data")?;
    let product = array(&value, "products")
        .iter()
        .find(|p| {
            same_title(&text(p, "title"), &game.title)
                || array(p, "variants")
                    .iter()
                    .any(|v| text(v, "id") == game.id)
        })
        .ok_or("This EA edition is not in the public catalog for this market")?;
    Ok(product.clone())
}

pub(crate) fn ea_cover(client: &Client, game: &LibraryGame) -> Result<String, String> {
    let product = ea_product(client, game)?;
    let images = array(&product, "media")
        .iter()
        .filter(|v| text(v, "type") == "image")
        .map(|v| text(v, "url"))
        .filter(|url| safe_url(url))
        .collect::<Vec<_>>();
    images
        .iter()
        .find(|url| url.contains("9x16") || url.contains("pack-front"))
        .or_else(|| images.first())
        .cloned()
        .ok_or("EA returned no artwork".into())
}

pub(super) fn ea(client: &Client, game: &LibraryGame, out: &mut Details) -> Result<(), String> {
    let product = ea_product(client, game)?;
    let product = &product;
    out.set_description(text(&product["description"], "html"));
    if out.description.is_empty() {
        out.set_description(text(&product["description"], "plain"));
    }
    out.released = text(&product["metadata"], "release_date");
    for m in array(product, "media") {
        let url = text(m, "url");
        if safe_url(&url) {
            out.media.push(Media {
                preview: url.clone(),
                url,
                title: text(m, "alt_text"),
                video: text(m, "type") == "video",
            });
        }
    }
    let id = text(product, "id");
    out.store_url = Some(format!(
        "https://store.ea.com/commerce/app/products/{}",
        crate::catalog::encode(&id)
    ));
    let html = get_html(
        client,
        &format!(
            "https://store.ea.com/commerce/app/products/{}",
            crate::catalog::encode(&id)
        ),
    )?;
    parse_ea_html(&html, out);
    out.sources.push("EA Store".into());
    Ok(())
}
fn parse_ea_html(html: &str, out: &mut Details) {
    let document = Html::parse_document(html);
    for group in document.select(&selector(".pdp-sysreq-group")) {
        let platform = group
            .select(&selector(".pdp-sysreq-platform"))
            .next()
            .map(|n| requirement_text(&n.inner_html()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Windows".into());
        let columns = group
            .select(&selector(".pdp-sysreq-col"))
            .map(|n| requirement_text(&n.inner_html()))
            .collect::<Vec<_>>();
        out.requirements.push(Requirement {
            platform,
            minimum: columns.first().cloned().unwrap_or_default(),
            recommended: columns.get(1).cloned().unwrap_or_default(),
        });
    }
    parse_schema(&document, out);
    if let Some(about) = document.select(&selector(".pdp-about")).next() {
        out.set_description(about.inner_html());
    }
    collect_html_media(&document, "https://store.ea.com", out);
}

pub(super) fn ubisoft(
    client: &Client,
    game: &LibraryGame,
    out: &mut Details,
) -> Result<(), String> {
    let url = store_url(game)
        .filter(|s| s.contains(".html"))
        .map(str::to_owned);
    let url = if let Some(url) = url {
        url
    } else {
        let html = get_html(
            client,
            &format!(
                "https://store.ubisoft.com/us/search?q={}&lang=en_US",
                crate::catalog::encode(&game.title)
            ),
        )?;
        let document = Html::parse_document(&html);
        let slug = game
            .title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let direct = document
            .select(&selector("a[href]"))
            .filter_map(|n| n.value().attr("href"))
            .find(|href| href.contains(&format!("/{slug}/")) && href.contains(".html"))
            .map(|s| absolute(s, "https://store.ubisoft.com"));
        if let Some(url) = direct {
            url
        } else {
            let key = html
                .split_once("apiKey: \"")
                .and_then(|(_, s)| s.split('"').next())
                .ok_or("Ubisoft search configuration is missing")?;
            let app = html
                .split_once("appId: \"")
                .and_then(|(_, s)| s.split('"').next())
                .ok_or("Ubisoft search application is missing")?;
            if !app.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err("Invalid Ubisoft search configuration".into());
            }
            let request=client.get(format!("https://{}-dsn.algolia.net/1/indexes/production__us_ubisoft__products__en_US__best_sellers?query={}&hitsPerPage=20",app.to_lowercase(),crate::catalog::encode(&game.title))).header("X-Algolia-Application-Id",app).header("X-Algolia-API-Key",key);
            let value: Value = serde_json::from_slice(&response(request)?)
                .map_err(|_| "Ubisoft returned invalid search results")?;
            let hit = array(&value, "hits")
                .iter()
                .find(|v| same_title(&text(v, "name"), &game.title))
                .ok_or("This Ubisoft edition could not be matched to its store page")?;
            absolute(&text(hit, "url"), "https://store.ubisoft.com")
        }
    };
    if !url.starts_with("https://store.ubisoft.com/") {
        return Err("Invalid Ubisoft store page".into());
    }
    let html = get_html(client, &url)?;
    out.store_url = Some(url);
    parse_ubisoft(&html, out)?;
    out.sources.push("Ubisoft Store".into());
    Ok(())
}
fn parse_ubisoft(html: &str, out: &mut Details) -> Result<(), String> {
    let model = embedded_json(html, "window.extendedProductModel")
        .ok_or("Ubisoft returned no product information")?;
    out.set_description(text(&model["summary"], "productDescription"));
    out.released = text(&model["details"], "releaseDateString");
    out.age_rating = text(&model["rating"], "ageRating").to_uppercase();
    for item in array(&model["media"], "media_slider_PD") {
        let url = absolute(&text(item, "url"), "https://store.ubisoft.com");
        out.media.push(Media {
            preview: url.clone(),
            url,
            title: "Screenshot".into(),
            video: false,
        });
    }
    let document = Html::parse_document(html);
    parse_schema(&document, out);
    for dt in document.select(&selector("dt")) {
        let label = clean(&dt.inner_html()).to_lowercase();
        let value = dt
            .next_siblings()
            .filter_map(scraper::ElementRef::wrap)
            .find(|n| n.value().name() == "dd")
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        if label.starts_with("developer") {
            out.developers = vec![value.clone()];
        }
        if label.starts_with("publisher") {
            out.publishers = vec![value];
        }
    }
    let mut requirements = Requirement {
        platform: "Windows".into(),
        ..Default::default()
    };
    for panel in document.select(&selector(
        ".experience-pdp-systemRequirementsTab .tabs-panel",
    )) {
        let label = panel
            .select(&selector(".tabs__button"))
            .next()
            .map(|n| clean(&n.inner_html()).to_lowercase())
            .unwrap_or_default();
        let value = panel
            .select(&selector("dl"))
            .map(|n| requirement_text(&n.inner_html()))
            .collect::<Vec<_>>()
            .join("\n");
        if label.contains("minimum") && requirements.minimum.is_empty() {
            requirements.minimum = value;
        } else if label.contains("recommend") && requirements.recommended.is_empty() {
            requirements.recommended = value;
        }
    }
    if !requirements.minimum.is_empty() || !requirements.recommended.is_empty() {
        out.requirements.push(requirements);
    }
    collect_html_media(&document, "https://store.ubisoft.com", out);
    Ok(())
}
fn parse_schema(document: &Html, out: &mut Details) {
    for script in document.select(&selector("script[type='application/ld+json']")) {
        if let Ok(v) = serde_json::from_str::<Value>(&script.inner_html()) {
            schema_node(&v, out);
        }
    }
    if out.description.is_empty() {
        let description = document
            .select(&selector("meta[name='description']"))
            .next()
            .and_then(|n| n.value().attr("content"))
            .unwrap_or_default()
            .to_owned();
        out.set_description(description);
    }
}
fn schema_node(v: &Value, out: &mut Details) {
    if let Some(items) = v.as_array() {
        for item in items {
            schema_node(item, out);
        }
        return;
    }
    if let Some(items) = v.get("@graph") {
        schema_node(items, out);
    }
    let kind = text(v, "@type");
    if !matches!(
        kind.as_str(),
        "VideoGame" | "Product" | "SoftwareApplication"
    ) {
        return;
    }
    if out.description.is_empty() {
        out.set_description(text(v, "description"));
    }
    if out.developers.is_empty() {
        let name = text(&v["author"], "name");
        if !name.is_empty() {
            out.developers.push(name);
        }
    }
    if out.publishers.is_empty() {
        let name = text(&v["publisher"], "name");
        if !name.is_empty() {
            out.publishers.push(name);
        }
    }
    if out.age_rating.is_empty() {
        out.age_rating = text(v, "contentRating");
    }
}
fn collect_html_media(document: &Html, base: &str, out: &mut Details) {
    for n in document.select(&selector(
        "[data-videoid],video source,video[src],iframe[src*='youtube.com/embed']",
    )) {
        let url = if let Some(id) = n.value().attr("data-videoid") {
            format!("https://www.youtube.com/watch?v={id}")
        } else {
            absolute(n.value().attr("src").unwrap_or_default(), base)
        };
        if safe_url(&url) && !out.media.iter().any(|m| m.url == url) {
            out.media.insert(
                0,
                Media {
                    url,
                    preview: String::new(),
                    title: "Trailer".into(),
                    video: true,
                },
            );
        }
    }
    if base.contains("ea.com") {
        for n in document.select(&selector("img[src]")) {
            let url = absolute(n.value().attr("src").unwrap_or_default(), base);
            if url.contains("app-images.ea.com") && !out.media.iter().any(|m| m.url == url) {
                out.media.push(Media {
                    preview: url.clone(),
                    url,
                    title: "Artwork".into(),
                    video: false,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steam_requirements_drop_client_footnote_and_marker() {
        assert_eq!(
            requirement_text(
                "<b>OS *:</b> Windows 7<br>Memory: 4 GB<br>* Starting January 1st, 2024, the Steam Client will only support Windows 10 and later versions."
            ),
            "OS: Windows 7\nMemory: 4 GB"
        );
    }
    #[test]
    fn steam_details_support_streaming_trailers_and_new_review_response() {
        let data = json!({"detailed_description":"<p>A game</p>","developers":["Studio"],"publishers":["Publisher"],"ratings":{"esrb":{"rating":"T"}},"movies":[{"name":"Trailer","hls_h264":"https://cdn.example/trailer.m3u8"}],"screenshots":[{"path_full":"https://cdn.example/shot.jpg"}],"pc_requirements":{"minimum":"<b>OS:</b> Windows 10<br>RAM: 8GB"},"linux_requirements":{"minimum":"<b>Minimum:</b>"}});
        let mut out = Details::default();
        parse_steam(&data, &mut out);
        assert_eq!(out.description_html.as_deref(), Some("<p>A game</p>"));
        assert_eq!(out.developers, vec!["Studio"]);
        assert_eq!(out.media.len(), 2);
        assert!(out.media[0].video);
        assert!(out.requirements[0].minimum.contains("RAM: 8GB"));
        assert_eq!(out.age_rating, "ESRB T");
        assert_eq!(out.requirements.len(), 1);
        parse_steam_reviews(
            &json!({"query_summary":{"total_reviews":2,"total_positive":1,"review_score_desc":"Mixed"},"reviews":[{"author":{"playtime_forever":120},"review":"[b]Great[/b]","voted_up":true,"timestamp_created":1750032000}]}),
            &mut out,
        );
        assert_eq!(out.reviews[0].text, "Great");
        assert_eq!(out.reviews[0].hours, Some(2.));
        assert_eq!(out.reviews[0].date, "2025.06.16");
        assert_eq!(out.reviews[0].recommended(), Some(true));
        assert_eq!(out.score(), Some(2.5));
        assert_eq!(out.review_source, "Steam players");
        assert!(out.review_summary.contains("2 reviews"));
    }
    #[test]
    fn epic_metadata_preserves_platform_requirements() {
        let mut out = Details::default();
        parse_epic(
            &json!({"about":{"description":"# Game\n\nA **bold** game.\n\n![Art](https://cdn.example/art.jpg)","developerAttribution":"Studio"},"requirements":{"systems":[{"systemType":"Windows","details":[{"title":"Memory","minimum":"4GB","recommended":"8GB"}]}]},"carousel":{"items":[{"image":{"src":"https://cdn.example/a.jpg"}}]}}),
            &mut out,
        );
        assert_eq!(out.requirements[0].recommended, "Memory: 8GB");
        assert_eq!(out.media.len(), 1);
        let html = out.description_html.as_deref().unwrap();
        assert!(html.contains("<h1>Game</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<img src=\"https://cdn.example/art.jpg\""));
    }
    #[test]
    fn epic_portrait_and_requirement_groups_keep_their_roles() {
        let data = json!({
            "about": {"image": {"src": "https://cdn.example/portrait.jpg"}},
            "carousel": {"items": [{"image": {"src": "https://cdn.example/screenshot.jpg"}}]},
            "requirements": {"systems": [
                {"systemType": "Windows", "details": [{"title": "GPU ", "minimum": " Integrated ", "recommended": "Dedicated"}]},
                {"systemType": "Windows - Epic Quality Presets", "details": [{"title": "GPU", "recommended": "RTX 3070"}]},
                {"systemType": "Windows - UEFN", "details": [{"title": "Memory", "minimum": "16 GB", "recommended": "32 GB"}]}
            ]}
        });
        assert_eq!(
            epic_portrait(&data).as_deref(),
            Some("https://cdn.example/portrait.jpg")
        );
        assert!(epic_portrait(&json!({"carousel":data["carousel"]})).is_none());
        assert!(epic_portrait(&json!({"about":{"image":{"src":"javascript:bad()"}}})).is_none());
        let mut out = Details::default();
        parse_epic(&data, &mut out);
        assert_eq!(out.requirements.len(), 3);
        assert_eq!(out.requirements[0].minimum, "GPU: Integrated");
        assert_eq!(
            out.requirements[1].platform,
            "Windows - Epic Quality Presets"
        );
        assert!(out.requirements[1].minimum.is_empty());
        assert_eq!(out.requirements[2].platform, "Windows - UEFN");
    }
    #[test]
    fn ea_and_ubisoft_parse_public_html_without_executing_scripts() {
        let mut ea = Details::default();
        parse_ea_html(
            "<p class='pdp-about'>An <strong>adventure</strong>.</p><div class='pdp-sysreq-group'><div class='pdp-sysreq-platform'>Windows</div><div class='pdp-sysreq-col'><h4>Minimum</h4><dl><dt>RAM</dt><dd>8 GB</dd></dl></div><div class='pdp-sysreq-col'>16 GB</div></div>",
            &mut ea,
        );
        assert_eq!(ea.requirements.len(), 1);
        assert_eq!(ea.requirements[0].recommended, "16 GB");
        assert!(
            ea.description_html
                .as_deref()
                .unwrap()
                .contains("<strong>adventure</strong>")
        );
        let mut ubi = Details::default();
        parse_ubisoft(r#"<script>window.extendedProductModel = {"summary":{"productDescription":"<p>Adventure</p>"},"rating":{"ageRating":"esrb-t"},"media":{"media_slider_PD":[{"url":"/screen.jpg"}]}};</script><dl><dt>Developer:</dt><dd>Studio</dd></dl><div data-videoid="trailer-id"></div>"#,&mut ubi).unwrap();
        assert_eq!(ubi.description, "Adventure");
        assert_eq!(ubi.description_html.as_deref(), Some("<p>Adventure</p>"));
        assert_eq!(ubi.developers, vec!["Studio"]);
        assert!(ubi.media.iter().any(|m| m.video));
        assert!(
            ubi.media
                .iter()
                .any(|m| m.url == "https://store.ubisoft.com/screen.jpg")
        );
    }
    #[test]
    fn microsoft_uses_pc_sku_requirements_and_labels_customer_reviews() {
        let mut out = Details::default();
        parse_xbox(
            &json!({"LocalizedProperties":[{"DeveloperName":"Studio","ProductDescription":"Description\r\n\r\nFeatures","CMSVideos":[{"HLS":"https://cdn.example/video.m3u8"}]}],"DisplaySkuAvailabilities":[{"Sku":{"Properties":{"HardwareProperties":{"MinimumProcessor":"CPU","MinimumHardware":["m08","unknown-code"],"RecommendedGraphics":"GPU"}}}}]}),
            &mut out,
        );
        assert!(out.requirements[0].minimum.contains("Memory: 8 GB"));
        assert!(!out.requirements[0].minimum.contains("unknown-code"));
        assert_eq!(out.requirements[0].recommended, "Graphics: GPU");
        out.normalize();
        assert_eq!(out.description_blocks.len(), 2);
        parse_xbox_reviews(
            &json!({"totalReviews":1,"reviews":[{"userName":"Player","reviewText":"Good","rating":4,"deviceFamily":"Windows.Xbox"}]}),
            &mut out,
        );
        assert_eq!(out.reviews[0].rating, "4/5");
        assert_eq!(out.review_source, "Microsoft Store customers");
        assert_eq!(out.reviews[0].platform, "Console");
        assert_eq!(out.reviews[0].recommended(), Some(true));
    }
}

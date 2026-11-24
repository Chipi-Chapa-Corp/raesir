use super::*;
use crate::details::{Details, Media, ModMetadata, ModPost, display_date, rich};
use serde_json::Value;

const SITE: &str = "https://gamebanana.com";
const SOURCE: &str = "GameBanana";
const DEADLOCK: u64 = 20948;

#[derive(Clone, Copy)]
enum Listing {
    Mod,
    Sound,
}

impl Listing {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "mods" => Some(Self::Mod),
            "sounds" => Some(Self::Sound),
            _ => None,
        }
    }

    fn model(self) -> &'static str {
        match self {
            Self::Mod => "Mod",
            Self::Sound => "Sound",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::Sound => "sounds",
        }
    }

    fn key(self, id: u64) -> String {
        format!("gamebanana:{}:{id}", self.path())
    }

    fn url(self, id: u64) -> String {
        format!("{SITE}/{}/{id}", self.path())
    }
}

impl Manager {
    pub fn gamebanana_game(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> Result<Option<u64>, String> {
        if !can_discover_mods(game) {
            return Ok(None);
        }
        if game.store == "steam" {
            match game.id.as_str() {
                "1422450" => return Ok(Some(DEADLOCK)),
                "570" => return Ok(Some(4874)),
                "730" => return Ok(Some(18134)),
                _ => {}
            }
        }
        let title = providers::normalized_title(&game.title);
        if title.is_empty() {
            return Ok(None);
        }
        let url = format!(
            "{SITE}/apiv11/Util/Search/Results?_sModelName=Game&_sSearchString={}",
            crate::catalog::encode(&game.title)
        );
        let response = self.gamebanana_json(&url, refresh)?;
        let mut matches = records(&response)?.iter().filter(|record| {
            record["_sModelName"] == "Game"
                && providers::normalized_title(string(&record["_sName"])) == title
        });
        let first = matches.next().and_then(|item| item["_idRow"].as_u64());
        Ok(first.filter(|id| matches.all(|item| item["_idRow"].as_u64() == Some(*id))))
    }

    pub fn gamebanana_page(
        &self,
        game: u64,
        query: &str,
        page: u32,
        order: CatalogOrder,
        refresh: bool,
    ) -> Result<CatalogPage, String> {
        let (mods, sounds) = std::thread::scope(|scope| {
            let sounds = scope.spawn(|| {
                self.gamebanana_listings(Listing::Sound, game, query, page, order, refresh)
            });
            let mods = self.gamebanana_listings(Listing::Mod, game, query, page, order, refresh);
            (
                mods,
                sounds
                    .join()
                    .unwrap_or_else(|_| Err("Sound catalog worker stopped".into())),
            )
        });
        match (mods, sounds) {
            (Ok(mut mods), Ok(mut sounds)) => {
                mods.next = mods.next.or(sounds.next);
                mods.packages.append(&mut sounds.packages);
                Ok(mods)
            }
            (Ok(mut page), Err(error)) => {
                page.warnings.push(format!("GameBanana sounds: {error}"));
                Ok(page)
            }
            (Err(error), Ok(mut page)) => {
                page.warnings.push(format!("GameBanana mods: {error}"));
                Ok(page)
            }
            (Err(error), Err(_)) => Err(error),
        }
    }

    fn gamebanana_listings(
        &self,
        listing: Listing,
        game: u64,
        query: &str,
        page: u32,
        order: CatalogOrder,
        refresh: bool,
    ) -> Result<CatalogPage, String> {
        let model = listing.model();
        let url = if query.is_empty() {
            let sort = match order {
                CatalogOrder::All => "Generic_LatestModified",
                CatalogOrder::Trending => "Generic_NewAndUpdated",
                CatalogOrder::Downloads => "Generic_MostDownloaded",
            };
            format!(
                "{SITE}/apiv11/{model}/Index?_aFilters%5BGeneric_Game%5D={game}&_sSort={sort}&_nPerpage=24&_nPage={page}"
            )
        } else {
            format!(
                "{SITE}/apiv11/Util/Search/Results?_idGameRow={game}&_sModelName={model}&_nPage={page}&_sSearchString={}",
                crate::catalog::encode(query)
            )
        };
        let data = if query.is_empty() && page == 1 {
            self.gamebanana_json(&url, refresh)?
        } else {
            providers::read_json(|| {
                Ok(providers::http()?
                    .get(&url)
                    .timeout(Duration::from_secs(10)))
            })?
        };
        let rows = records(&data)?;
        let packages = rows
            .iter()
            .filter_map(|row| package(row, game, listing))
            .collect();
        let meta = &data["_aMetadata"];
        let next =
            (!rows.is_empty() && meta["_bIsComplete"].as_bool() == Some(false)).then(|| page + 1);
        Ok(CatalogPage {
            packages,
            next,
            ..Default::default()
        })
    }

    pub(super) fn gamebanana_details(&self, id: &str) -> Result<Details, String> {
        let (listing, id) = listing_id(id)?;
        let data = self.gamebanana_listing(listing, id, false)?;
        let name = data["_sName"]
            .as_str()
            .ok_or("GameBanana mod was not found")?;
        let page = listing.url(id);
        let blocks = rich::parse(string(&data["_sText"]), &page);
        let description = blocks
            .iter()
            .find_map(|block| match block {
                rich::Block::Text(text) => Some(
                    text.spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .and_then(|text| {
                text.lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let media = images(&data).collect();
        let (changelog, comments) = std::thread::scope(|scope| {
            let updates = scope.spawn(|| self.gamebanana_posts(listing, id, true, &data));
            let comments = self.gamebanana_posts(listing, id, false, &data);
            (updates.join().unwrap_or_default(), comments)
        });
        Ok(Details {
            key: listing.key(id),
            title: name.into(),
            store_url: Some(page.clone()),
            description,
            description_blocks: blocks,
            developers: vec![string(&data["_aSubmitter"]["_sName"]).into()],
            genres: categories(&data),
            released: date(&data["_tsDateAdded"]),
            media,
            mod_metadata: Some(ModMetadata {
                version: string(&data["_sVersion"]).into(),
                last_updated: date(
                    data.get("_tsDateUpdated")
                        .unwrap_or(&data["_tsDateModified"]),
                ),
                likes: data["_nLikeCount"].as_u64(),
                downloads: data["_nDownloadCount"].as_u64(),
                views: data["_nViewCount"].as_u64(),
                changelog,
                comments,
                ..Default::default()
            }),
            sources: vec![SOURCE.into()],
            ..Default::default()
        })
    }

    fn gamebanana_posts(
        &self,
        listing: Listing,
        id: u64,
        updates: bool,
        data: &Value,
    ) -> Vec<ModPost> {
        let (endpoint, count) = if updates {
            ("Updates", "_nUpdatesCount")
        } else {
            ("Posts", "_nPostCount")
        };
        if data[count].as_u64() == Some(0) {
            return Vec::new();
        }
        let page = listing.url(id);
        let url = format!(
            "{SITE}/apiv11/{}/{id}/{endpoint}?_nPerpage=10",
            listing.model()
        );
        self.gamebanana_json(&url, false)
            .ok()
            .and_then(|response| {
                Some(
                    records(&response)
                        .ok()?
                        .iter()
                        .take(10)
                        .filter_map(|row| {
                            let title = string(&row["_sName"]).to_owned();
                            let source = string(&row["_sText"]);
                            let (mut text, links) = if updates {
                                (providers::plain_text(source), Vec::new())
                            } else {
                                crate::details::comments::formatted_text(source, &page)
                            };
                            for change in row["_aChangeLog"].as_array().into_iter().flatten() {
                                if let Some(line) = change["text"].as_str() {
                                    if !text.is_empty() {
                                        text.push('\n');
                                    }
                                    text.push_str(line);
                                }
                            }
                            if title.is_empty() && text.is_empty() {
                                return None;
                            }
                            let author = string(
                                &row[if updates { "_aSubmitter" } else { "_aPoster" }]["_sName"],
                            );
                            Some(ModPost {
                                title,
                                author: author.into(),
                                date: date(&row["_tsDateAdded"]),
                                text,
                                links,
                            })
                        })
                        .collect(),
                )
            })
            .unwrap_or_default()
    }

    pub(super) fn deadlock_files(&self, package: &str) -> Result<Vec<ModFile>, String> {
        let data = self.deadlock_listing(package, false)?;
        Ok(listing_files(&data))
    }

    pub(super) fn deadlock_download(
        &self,
        package: &str,
        file: u64,
        cancelled: impl Fn() -> bool,
    ) -> Result<(InstalledMod, tempfile::NamedTempFile), String> {
        let data = self.deadlock_listing(package, true)?;
        if !listing_files(&data).iter().any(|option| option.id == file) {
            return Err("This file is unavailable for the selected Deadlock mod".into());
        }
        if crate::http::network_disabled() {
            return Err("Mod networking is disabled".into());
        }
        let client = reqwest::blocking::Client::builder()
            .user_agent(crate::http::USER_AGENT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5
                    && attempt.url().scheme() == "https"
                    && attempt.url().host_str().is_some_and(|host| {
                        host == "gamebanana.com" || host.ends_with(".gamebanana.com")
                    })
                {
                    attempt.follow()
                } else {
                    attempt.error("Unexpected GameBanana download redirect")
                }
            }))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|error| error.to_string())?;
        let mut response = client
            .get(format!("{SITE}/dl/{file}"))
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("GameBanana download failed: {error}"))?;
        const MAX: u64 = 1024 * 1024 * 1024;
        if response.content_length().is_some_and(|bytes| bytes > MAX) {
            return Err("GameBanana archive exceeds 1 GiB".into());
        }
        use std::io::{Read, Write};
        let mut archive = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let mut buffer = [0; 64 * 1024];
        let mut total = 0;
        loop {
            if cancelled() {
                return Err("Mod installation cancelled".into());
            }
            let bytes = response
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if bytes == 0 {
                break;
            }
            total += bytes as u64;
            if total > MAX {
                return Err("GameBanana archive exceeds 1 GiB".into());
            }
            archive
                .write_all(&buffer[..bytes])
                .map_err(|error| error.to_string())?;
        }
        if total == 0 {
            return Err("GameBanana returned an empty archive".into());
        }
        let package = InstalledMod {
            id: package.into(),
            name: string(&data["_sName"]).into(),
            owner: string(&data["_aSubmitter"]["_sName"]).into(),
            version: string(&data["_sVersion"]).into(),
            enabled: true,
            local: false,
            dependencies: Vec::new(),
            automatic: false,
        };
        Ok((package, archive))
    }

    fn deadlock_listing(&self, package: &str, refresh: bool) -> Result<Value, String> {
        let (listing, id) = listing_id(package)?;
        let data = self.gamebanana_listing(listing, id, refresh)?;
        if data["_aGame"]["_idRow"].as_u64() != Some(DEADLOCK)
            || data["_idRow"].as_u64() != Some(id)
        {
            return Err("This GameBanana listing does not belong to Deadlock".into());
        }
        Ok(data)
    }

    fn gamebanana_listing(
        &self,
        listing: Listing,
        id: u64,
        refresh: bool,
    ) -> Result<Value, String> {
        let model = listing.model();
        self.gamebanana_json(&format!("{SITE}/apiv11/{model}/{id}/ProfilePage"), refresh)
    }

    fn gamebanana_json(&self, url: &str, refresh: bool) -> Result<Value, String> {
        self.provider_json_inner(url, refresh, true, || {
            providers::read_json(|| {
                Ok(providers::http()?.get(url).timeout(Duration::from_secs(10)))
            })
        })
    }
}

fn records(data: &Value) -> Result<&[Value], String> {
    data["_aRecords"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "Invalid GameBanana catalog".into())
}

fn string(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn date(value: &Value) -> String {
    value
        .as_u64()
        .filter(|&value| value > 0)
        .map_or_else(String::new, |value| display_date(&value.to_string()))
}

fn categories(data: &Value) -> Vec<String> {
    ["_aRootCategory", "_aSubCategory"]
        .into_iter()
        .filter_map(|key| data[key]["_sName"].as_str().map(str::to_owned))
        .collect()
}

fn images(data: &Value) -> impl Iterator<Item = Media> {
    data["_aPreviewMedia"]["_aImages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let base = image["_sBaseUrl"].as_str()?;
            let image_url = |filename: &str| {
                let url = reqwest::Url::parse(&format!("{base}/{filename}")).ok()?;
                (url.scheme() == "https" && url.host_str() == Some("images.gamebanana.com"))
                    .then(|| url.to_string())
            };
            let url = image_url(image["_sFile"].as_str()?)?;
            let preview = image["_sFile530"]
                .as_str()
                .and_then(image_url)
                .unwrap_or_else(|| url.clone());
            Some(Media {
                url,
                preview,
                title: string(&image["_sCaption"]).into(),
                video: false,
            })
        })
}

fn package(data: &Value, game: u64, listing: Listing) -> Option<Package> {
    if data["_sModelName"] != listing.model() || data["_aGame"]["_idRow"].as_u64() != Some(game) {
        return None;
    }
    let id = data["_idRow"].as_u64()?;
    let description = string(&data["_sDescription"]);
    Some(Package {
        id: listing.key(id),
        name: data["_sName"].as_str()?.into(),
        owner: string(&data["_aSubmitter"]["_sName"]).into(),
        description: description.into(),
        version: string(&data["_sVersion"]).into(),
        page: listing.url(id),
        icon: images(data)
            .next()
            .map_or_else(String::new, |image| image.preview),
        downloads: data["_nDownloadCount"].as_u64().unwrap_or_default(),
        deprecated: data["_bIsObsolete"].as_bool().unwrap_or_default(),
        categories: categories(data),
        source: SOURCE.into(),
    })
}

fn listing_id(package: &str) -> Result<(Listing, u64), String> {
    let (kind, id) = package
        .strip_prefix("gamebanana:")
        .and_then(|key| key.split_once(':'))
        .ok_or("Invalid GameBanana listing")?;
    let listing = Listing::from_key(kind).ok_or("Invalid GameBanana listing type")?;
    let id = id.parse::<u64>().map_err(|_| "Invalid GameBanana mod ID")?;
    Ok((listing, id))
}

fn listing_files(data: &Value) -> Vec<ModFile> {
    data["_aFiles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let id = file["_idRow"].as_u64()?;
            let name = file["_sFile"].as_str()?;
            let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
            if file["_bIsArchived"] == true
                || !matches!(extension.as_str(), "vpk" | "zip" | "rar" | "7z")
            {
                return None;
            }
            Some(ModFile {
                id,
                label: name.into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_listing_type_and_game_in_the_catalog() {
        let sound = json!({
            "_idRow": 41, "_sModelName": "Sound", "_sName": "A sound",
            "_aGame": {"_idRow": DEADLOCK}
        });
        let package = package(&sound, DEADLOCK, Listing::Sound).unwrap();
        assert_eq!(package.id, "gamebanana:sounds:41");
        assert_eq!(package.page, "https://gamebanana.com/sounds/41");
        assert!(super::package(&sound, DEADLOCK, Listing::Mod).is_none());
        assert!(super::package(&sound, 730, Listing::Sound).is_none());
    }

    #[test]
    fn lists_current_archives_without_requiring_a_manager_integration() {
        let data = json!({"_aFiles": [
            {"_idRow": 1559314, "_sFile": "pak52_dir_8245e.rar"},
            {"_idRow": 1559315, "_sFile": "old.zip", "_bIsArchived": true},
            {"_idRow": 1559316, "_sFile": "installer.exe"}
        ]});
        let files = listing_files(&data);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, 1559314);
    }
}

use super::*;
type CompanionDownload = Option<(PathBuf, String, Vec<String>)>;
pub(super) type Download = (
    Package,
    PathBuf,
    String,
    CompanionDownload,
    Option<SourceFile>,
);

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("ExposedGames/0.1")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())
}

impl Manager {
    pub(super) fn provider_json(&self, url: &str, refresh: bool) -> Result<Value, String> {
        self.provider_json_inner(url, refresh, true)
    }

    pub(in crate::mods) fn refresh_provider_json(&self, url: &str) -> Result<Value, String> {
        self.provider_json_inner(url, true, false)
    }

    fn provider_json_inner(&self, url: &str, refresh: bool, stale: bool) -> Result<Value, String> {
        let path = self
            .cache
            .join("providers")
            .join(format!("{}.json", launch::hash(url.as_bytes())));
        let cached = || {
            fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
        };
        let fresh = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(1800));
        if !refresh
            && fresh
            && let Some(value) = cached()
        {
            return Ok(value);
        }
        let fetch = || {
            if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
                return Err("Mod networking is disabled".to_owned());
            }
            let mut data = Vec::new();
            http()?
                .get(url)
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())?;
            if data.len() > 16 * 1024 * 1024 {
                return Err("Provider response is too large".into());
            }
            let value: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
            crate::storage::write_json(&path, &value)?;
            Ok(value)
        };
        fetch().or_else(|error| {
            if stale {
                cached().ok_or(error)
            } else {
                Err(error)
            }
        })
    }

    fn download(&self, url: &str) -> Result<PathBuf, String> {
        self.download_versioned(url, "")
    }

    fn download_versioned(&self, url: &str, revision: &str) -> Result<PathBuf, String> {
        let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
        if parsed.scheme() != "https" {
            return Err("Mod downloads require HTTPS".into());
        }
        let root = self.cache.join("downloads");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let identity = if revision.is_empty() {
            url.to_owned()
        } else {
            format!("{url}\n{revision}")
        };
        let path = root.join(launch::hash(identity.as_bytes()));
        if path.is_file() {
            return Ok(path);
        }
        if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
            return Err("Mod networking is disabled".into());
        }
        let response = http()?
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        if response.url().scheme() != "https" {
            return Err("Mod download redirected away from HTTPS".into());
        }
        let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
        let bytes = std::io::copy(&mut response.take(512 * 1024 * 1024 + 1), &mut temp)
            .map_err(|e| e.to_string())?;
        if bytes > 512 * 1024 * 1024 {
            return Err("Mod download exceeds 512 MiB".into());
        }
        temp.persist(&path).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub(in crate::mods) fn noita_catalog(
        &self,
        game: &LibraryGame,
        refresh: bool,
    ) -> Result<(Arc<Vec<Package>>, Vec<String>), String> {
        let definition = recipes::for_game(game).ok_or("Unsupported game")?;
        let mut packages = definition
            .mods
            .iter()
            .map(recipe_package)
            .collect::<Vec<_>>();
        let mut warnings = Vec::new();
        let result = (|| {
            let url = format!(
                "https://api.modworkshop.net/games/{}/mods",
                definition.modworkshop
            );
            let first = self.provider_json(&url, refresh)?;
            let count = first["meta"]["last_page"].as_u64().unwrap_or(1).min(100);
            let mut pages = vec![first];
            for group in (2..=count).collect::<Vec<_>>().chunks(4) {
                let fetched = std::thread::scope(|scope| {
                    group
                        .iter()
                        .map(|page| {
                            let url = format!("{url}?page={page}");
                            scope.spawn(move || self.provider_json(&url, refresh))
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|t| {
                            t.join()
                                .map_err(|_| "Mod catalog worker stopped".to_owned())?
                        })
                        .collect::<Result<Vec<_>, String>>()
                })?;
                pages.extend(fetched);
            }
            for page in pages {
                for value in page["data"]
                    .as_array()
                    .ok_or("Invalid ModWorkshop catalog")?
                {
                    if value["suspended"].as_bool() == Some(true) {
                        continue;
                    }
                    packages.push(modworkshop_package(value)?);
                }
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            warnings.push(format!("ModWorkshop could not load: {error}"));
        }
        if let Some(profile) = self.own_path(&definition.id) {
            for entry in read(&profile)? {
                if !packages.iter().any(|p| p.id == entry.package.id) {
                    packages.push(entry.package);
                }
            }
        }
        Ok((Arc::new(packages), warnings))
    }

    pub(super) fn download_recipe(&self, recipe: &recipes::Recipe) -> Result<Download, String> {
        let host = if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            return Err("This recipe does not support your OS".into());
        };
        let companion = recipe.companions.get(host);
        if !recipe.companions.is_empty() && companion.is_none() {
            return Err("This recipe has no companion for your OS".into());
        }
        let release = self.provider_json(
            &format!(
                "https://api.github.com/repos/{}/releases/latest",
                recipe.repository
            ),
            false,
        )?;
        let assets = release["assets"]
            .as_array()
            .ok_or("Release has no downloadable assets")?;
        let asset = |name: &str| -> Result<&str, String> {
            assets
                .iter()
                .find(|a| a["name"].as_str() == Some(name))
                .and_then(|a| a["browser_download_url"].as_str())
                .ok_or_else(|| format!("The release is missing {name}"))
        };
        let mod_path = self.download(asset(&recipe.mod_asset)?)?;
        let tool = if let Some(companion) = companion {
            Some((
                self.download(asset(&companion.asset)?)?,
                companion.executable.clone(),
                companion.arguments.clone(),
            ))
        } else {
            None
        };
        let mut package = recipe_package(recipe);
        package.version = release["tag_name"]
            .as_str()
            .ok_or("Release is missing its version")?
            .into();
        Ok((package, mod_path, recipe.folder.clone(), tool, None))
    }

    pub(super) fn download_modworkshop(
        &self,
        game: &recipes::Game,
        id: &str,
        selected_file: Option<u64>,
    ) -> Result<Download, String> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid ModWorkshop identifier".into());
        }
        let value = self.provider_json(&format!("https://api.modworkshop.net/mods/{id}"), false)?;
        if value["game"]["short_name"].as_str() != Some(&game.modworkshop) {
            return Err("This mod belongs to a different game".into());
        }
        if value["disable_mod_managers"].as_bool() == Some(true) {
            return Err("This author requests installation from the mod page".into());
        }
        let files = self.provider_json(
            &format!("https://api.modworkshop.net/mods/{id}/files"),
            false,
        )?;
        let files = files["data"]
            .as_array()
            .ok_or("This mod has no hosted files")?;
        let file = if let Some(file) = selected_file {
            Some(
                files
                    .iter()
                    .find(|f| f["id"].as_u64() == Some(file))
                    .ok_or("The selected file is no longer listed for this mod")?,
            )
        } else if let Some(primary) = value["download_id"].as_u64() {
            files
                .iter()
                .find(|f| f["id"].as_u64() == Some(primary))
                .or_else(|| files.first())
        } else {
            files.first()
        }
        .ok_or("This mod links to an external download; open its mod page")?;
        let url = file["download_url"]
            .as_str()
            .ok_or("This file has no download URL")?;
        let mut package = modworkshop_package(&value)?;
        package.version = file["version"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("File {}", file["id"]));
        let folder = file["name"].as_str().unwrap_or("").to_owned();
        let source = SourceFile {
            id: file["id"].as_u64().ok_or("Missing file ID")?,
            name: file["name"].as_str().unwrap_or("").into(),
            kind: file["type"].as_str().unwrap_or("").into(),
            label: file["label"].as_str().unwrap_or("").into(),
            mod_type: file["mod_type"].as_str().unwrap_or("").into(),
            revision: file["updated_at"].as_str().unwrap_or("").into(),
            url: url.into(),
        };
        Ok((
            package,
            self.download_versioned(url, &source.revision)?,
            folder,
            None,
            Some(source),
        ))
    }

    pub fn mod_files(&self, game: &LibraryGame, id: &str) -> Result<Vec<ModFile>, String> {
        let definition = recipes::for_game(game).ok_or("This game does not use ModWorkshop")?;
        let id = id
            .strip_prefix("modworkshop:")
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or("Invalid ModWorkshop mod")?;
        let value = self.provider_json(&format!("https://api.modworkshop.net/mods/{id}"), false)?;
        if value["game"]["short_name"].as_str() != Some(&definition.modworkshop) {
            return Err("This mod belongs to another game".into());
        }
        let files = self.provider_json(
            &format!("https://api.modworkshop.net/mods/{id}/files"),
            false,
        )?;
        let files = files["data"].as_array().ok_or("Invalid file list")?;
        files
            .iter()
            .map(|f| {
                Ok(ModFile {
                    id: f["id"].as_u64().ok_or("Missing file ID")?,
                    label: format!(
                        "{} {} ({})",
                        f["name"].as_str().unwrap_or("Mod file"),
                        f["version"].as_str().unwrap_or(""),
                        f["type"].as_str().unwrap_or("archive")
                    ),
                })
            })
            .collect()
    }

    pub(in crate::mods) fn noita_details(
        &self,
        game: &LibraryGame,
        id: &str,
    ) -> Result<crate::details::Details, String> {
        let definition = recipes::for_game(game).ok_or("Unsupported game")?;
        let mut package = None;
        let mut markdown = None;
        if let Some(id) = id.strip_prefix("recipe:") {
            let recipe = definition
                .mods
                .iter()
                .find(|r| r.id == id)
                .ok_or("Unknown bundled mod")?;
            package = Some(recipe_package(recipe));
            markdown = Some(format!(
                "{}\n\nInstalls the mod and its matching networking proxy from the same upstream release. The proxy opens with a modded launch so you can host or join a lobby.",
                recipe.description
            ));
        } else if let Some(id) = id.strip_prefix("modworkshop:") {
            let value =
                self.provider_json(&format!("https://api.modworkshop.net/mods/{id}"), false)?;
            let mut text = value["desc"].as_str().unwrap_or("").to_owned();
            if let Some(instructions) = value["instructions"].as_str().filter(|s| !s.is_empty()) {
                text.push_str(&format!("\n\n## Installation\n\n{instructions}"));
            }
            package = Some(modworkshop_package(&value)?);
            markdown = Some(text);
        }
        if let Some(path) = self.own_path(&definition.id)
            && let Some(entry) = read(&path)?.into_iter().find(|e| e.package.id == id)
        {
            if let Some(current) = package.as_mut() {
                current.version = entry.package.version;
            } else {
                package = Some(entry.package);
            }
        }
        let p = package.ok_or("This mod is no longer available")?;
        let markdown = markdown.unwrap_or_else(|| p.description.clone());
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(&markdown));
        Ok(crate::details::Details {
            key: format!("{}:{id}", game.key),
            title: p.name,
            description: p.description,
            description_blocks: crate::details::rich::parse(&html, &p.page),
            description_html: Some(html),
            store_url: Some(p.page),
            developers: if p.owner.is_empty() {
                Vec::new()
            } else {
                vec![p.owner]
            },
            genres: p.categories,
            updated: crate::details::now(),
            ..Default::default()
        })
    }
}

fn recipe_package(recipe: &recipes::Recipe) -> Package {
    Package {
        id: format!("recipe:{}", recipe.id),
        name: recipe.name.clone(),
        owner: recipe.repository.split('/').next().unwrap_or("").into(),
        description: recipe.description.clone(),
        version: "Latest release".into(),
        page: format!("https://github.com/{}", recipe.repository),
        icon: recipe.icon.clone(),
        downloads: 0,
        deprecated: false,
        categories: recipe.categories.clone(),
        source: "Exposed Games catalog".into(),
    }
}

fn modworkshop_package(value: &Value) -> Result<Package, String> {
    let id = value["id"].as_u64().ok_or("Missing ModWorkshop ID")?;
    let icon = value["thumbnail"]["file"]
        .as_str()
        .map(|name| format!("https://storage.modworkshop.net/mods/images/{name}"))
        .unwrap_or_default();
    let description = value["short_desc"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["desc"].as_str())
        .unwrap_or("")
        .chars()
        .take(600)
        .collect();
    Ok(Package {
        id: format!("modworkshop:{id}"),
        name: value["name"].as_str().unwrap_or("Untitled mod").into(),
        owner: value["user"]["name"].as_str().unwrap_or("").into(),
        description,
        version: value["custom_version"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Latest file")
            .into(),
        page: format!("https://modworkshop.net/mod/{id}"),
        icon,
        downloads: value["downloads"].as_u64().unwrap_or(0),
        deprecated: false,
        categories: value["category"]["name"]
            .as_str()
            .map(|s| vec![s.into()])
            .unwrap_or_default(),
        source: "ModWorkshop".into(),
    })
}

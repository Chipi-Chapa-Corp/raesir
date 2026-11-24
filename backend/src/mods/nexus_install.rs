use super::*;
use std::io::Read;

const API: &str = "https://api.nexusmods.com/v1";
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;

pub enum NexusInstall {
    Installed(Snapshot),
    ChooseFile(Vec<ModFile>),
    BrowserDownload,
}

#[cfg(test)]
mod tests {
    use super::{File, default_file};

    fn file(file_id: u64, category_id: u32, primary: bool) -> File {
        File {
            file_id,
            category_id: Some(category_id),
            is_primary: Some(primary),
            name: "Fixture".into(),
            version: None,
        }
    }

    #[test]
    fn direct_install_uses_an_unambiguous_file_and_never_guesses_between_editions() {
        assert_eq!(
            default_file(&[file(1, 1, false), file(2, 2, false)]),
            Some(1)
        );
        assert_eq!(
            default_file(&[file(1, 1, false), file(2, 1, true)]),
            Some(2)
        );
        assert_eq!(default_file(&[file(1, 1, false), file(2, 1, false)]), None);
        assert_eq!(default_file(&[file(1, 1, true), file(2, 1, true)]), None);
        assert_eq!(default_file(&[]), None);
    }
}

#[derive(Deserialize)]
struct File {
    file_id: u64,
    name: String,
    version: Option<String>,
    category_id: Option<u32>,
    is_primary: Option<bool>,
}

fn default_file(files: &[File]) -> Option<u64> {
    let mut primary = files.iter().filter(|f| f.is_primary == Some(true));
    if let Some(file) = primary.next() {
        return primary.next().is_none().then_some(file.file_id);
    }
    let mut main = files.iter().filter(|f| f.category_id == Some(1));
    if let Some(file) = main.next() {
        return main.next().is_none().then_some(file.file_id);
    }
    None
}

impl Manager {
    pub fn install_from_nexus(
        &self,
        game: &LibraryGame,
        package: &str,
        name: Option<&str>,
        file: Option<u64>,
        cancelled: impl Fn() -> bool,
    ) -> Result<NexusInstall, String> {
        install::changes_allowed()?;
        if crate::http::network_disabled() || cancelled() {
            return Err("Nexus downloads are disabled in this session".into());
        }
        let recipe = recipes::for_game(game);
        let target = recipe
            .is_none()
            .then(|| modde::Target::resolve(game))
            .flatten();
        let domain = if let Some(recipe) = recipe {
            &recipe.nexus_domain
        } else if let Some(target) = &target {
            target.domain()
        } else {
            return Err("Raesir has no installer for this game's Nexus mods yet".into());
        };
        let id = nexus::package_id(package)?;
        let key = nexus_account::api_key()?;
        let api =
            crate::http::credentials().map_err(|_| "Could not prepare the Nexus Mods request")?;
        let request = |suffix: &str| {
            nexus::nexus_headers(api.get(format!("{API}/games/{domain}/mods/{id}/{suffix}")))
                .header("apikey", &key)
                .timeout(Duration::from_secs(20))
        };
        let file = match file {
            Some(file) if file > 0 => file,
            Some(_) => return Err("Invalid Nexus file identifier".into()),
            None => {
                #[derive(Deserialize)]
                struct Files {
                    files: Vec<File>,
                }
                let data = providers::read_json(|| Ok(request("files.json")))?;
                let mut files = serde_json::from_value::<Files>(data)
                    .map_err(|_| "Nexus Mods returned an invalid file list")?
                    .files;
                files.retain(|f| f.file_id > 0 && matches!(f.category_id, Some(1..=3)));
                if files.is_empty() {
                    return Err("Nexus Mods lists no installable files for this mod".into());
                }
                let Some(file) = default_file(&files) else {
                    return Ok(NexusInstall::ChooseFile(
                        files
                            .into_iter()
                            .map(|f| ModFile {
                                id: f.file_id,
                                label: match f.version.filter(|v| !v.is_empty()) {
                                    Some(version) => format!("{} · {version}", f.name),
                                    None => f.name,
                                },
                            })
                            .collect(),
                    ));
                };
                file
            }
        };
        let mut response = request(&format!("files/{file}/download_link.json"))
            .send()
            .map_err(|_| "Could not reach Nexus Mods for the download link")?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Ok(NexusInstall::BrowserDownload);
        }
        if !response.status().is_success() {
            return Err(format!(
                "Nexus Mods could not provide a download link ({})",
                response.status()
            ));
        }
        #[derive(Deserialize)]
        struct Link {
            #[serde(rename = "URI")]
            url: String,
        }
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(128 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read the Nexus Mods download link")?;
        let links: Vec<Link> = serde_json::from_slice(&bytes)
            .map_err(|_| "Nexus Mods returned an invalid download link")?;
        let link = links
            .into_iter()
            .filter_map(|entry| reqwest::Url::parse(&entry.url).ok())
            .find(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
            .ok_or("Nexus Mods did not provide a secure download link")?;
        if cancelled() {
            return Err("Nexus Mod installation cancelled".into());
        }
        // Use a separate client without the account key for the CDN request.
        let cdn = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() == "https" && attempt.previous().len() < 5 {
                    attempt.follow()
                } else {
                    attempt.error("Insecure or excessive redirects during mod download")
                }
            }))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(900))
            .build()
            .map_err(|_| "Could not prepare the Nexus Mods download")?;
        let mut response = cdn
            .get(link)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| "The Nexus Mods download failed. Try again.")?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ARCHIVE)
        {
            return Err("Nexus archive exceeds 1 GiB".into());
        }
        let mut archive = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let mut reader = (&mut response).take(MAX_ARCHIVE + 1);
        let mut total = 0;
        let mut buffer = [0; 64 * 1024];
        loop {
            if cancelled() {
                return Err("Nexus Mod installation cancelled".into());
            }
            let count = reader
                .read(&mut buffer)
                .map_err(|_| "The Nexus Mods download was interrupted")?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_ARCHIVE {
                return Err("Nexus archive exceeds 1 GiB".into());
            }
            use std::io::Write;
            archive
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        if total == 0 {
            return Err("Nexus Mods returned an empty archive".into());
        }
        if let Some(target) = target {
            self.import_modde_nexus(game, &target, id, archive.path(), name, cancelled)
                .map(NexusInstall::Installed)
        } else {
            let page = nexus::mod_url(domain, id);
            self.import_nexus(game, &page, archive.path(), "")
                .map(NexusInstall::Installed)
        }
    }
}

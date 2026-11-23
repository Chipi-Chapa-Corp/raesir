use super::*;
use serde_json::Value;
use std::io::Read;

pub(super) fn plain_text(html: &str) -> String {
    if !html.contains('<') && html.contains('&') {
        return scraper::Html::parse_fragment(html)
            .root_element()
            .text()
            .collect::<String>()
            .trim()
            .to_owned();
    }
    crate::details::rich::parse(html, "https://example.invalid/")
        .into_iter()
        .filter_map(|block| match block {
            crate::details::rich::Block::Text(text) => Some(
                text.spans
                    .into_iter()
                    .map(|span| span.text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn html(url: &str) -> Result<String, String> {
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
        return Err("Mod networking is disabled".into());
    }
    const LIMIT: u64 = 8 * 1024 * 1024;
    let response = http()?
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| error.to_string())?;
    let mut html = String::new();
    response
        .take(LIMIT + 1)
        .read_to_string(&mut html)
        .map_err(|error| error.to_string())?;
    if html.len() as u64 > LIMIT {
        return Err("Provider response is too large".into());
    }
    Ok(html)
}

pub(super) fn normalized_title(title: &str) -> String {
    title
        .replace(['™', '®'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("Raesir/0.1")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())
}

impl Manager {
    pub(super) fn provider_json(&self, url: &str, refresh: bool) -> Result<Value, String> {
        self.provider_json_inner(url, refresh, true, || read_json(|| Ok(http()?.get(url))))
    }

    pub(super) fn refresh_provider_json(&self, url: &str) -> Result<Value, String> {
        self.provider_json_inner(url, true, false, || read_json(|| Ok(http()?.get(url))))
    }

    pub(super) fn provider_json_inner(
        &self,
        key: &str,
        refresh: bool,
        stale: bool,
        fetch: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        let path = self
            .cache
            .join("providers")
            .join(format!("{}.json", launch::hash(key.as_bytes())));
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
            let value = fetch()?;
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
}

pub(super) fn read_json(
    request: impl FnOnce() -> Result<reqwest::blocking::RequestBuilder, String>,
) -> Result<Value, String> {
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() {
        return Err("Mod networking is disabled".into());
    }
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut data = Vec::new();
    request()?
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT {
        return Err("Provider response is too large".into());
    }
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}

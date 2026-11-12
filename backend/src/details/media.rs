use super::*;
use std::io::Write;

pub fn animation(url: &str, root: &Path, allow_download: bool) -> Result<PathBuf, String> {
    if !safe_url(url) {
        return Err("Unsupported animation URL".into());
    }
    let mut hash = DefaultHasher::new();
    url.hash(&mut hash);
    let path = root.join(format!("{:016x}.video", hash.finish()));
    if path.is_file() {
        return Ok(path);
    }
    if !allow_download || std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some() {
        return Err("Animation is not cached".into());
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut response = http()?
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| "Animation could not be downloaded")?
        .take(32 * 1024 * 1024 + 1);
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    let length = std::io::copy(&mut response, &mut temp).map_err(|e| e.to_string())?;
    if length == 0 || length > 32 * 1024 * 1024 {
        return Err("Animation is empty or too large".into());
    }
    temp.flush().map_err(|e| e.to_string())?;
    temp.persist(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_animations_work_offline_and_diagnostics_never_download() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://cdn.example/clip.webm";
        assert!(animation(url, dir.path(), false).is_err());
        assert!(animation("file:///etc/passwd", dir.path(), true).is_err());
        let mut hash = DefaultHasher::new();
        url.hash(&mut hash);
        let path = dir.path().join(format!("{:016x}.video", hash.finish()));
        fs::write(&path, b"cached video").unwrap();
        assert_eq!(animation(url, dir.path(), false).unwrap(), path);
    }
}

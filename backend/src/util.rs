//! Small helpers shared across scanners and providers.
use std::{
    io::{self, Read},
    time::{SystemTime, UNIX_EPOCH},
};

/// Current Unix time in seconds.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Reads all of `reader`, or `None` if it holds more than `limit` bytes.
pub fn read_limited(reader: impl Read, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// Like [`read_limited`], for UTF-8 text.
pub fn read_limited_text(reader: impl Read, limit: u64) -> io::Result<Option<String>> {
    let mut text = String::new();
    reader.take(limit + 1).read_to_string(&mut text)?;
    Ok((text.len() as u64 <= limit).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_reads_accept_the_limit_and_reject_one_byte_more() {
        assert_eq!(
            read_limited(&b"abcd"[..], 4).unwrap(),
            Some(b"abcd".to_vec())
        );
        assert_eq!(read_limited(&b"abcde"[..], 4).unwrap(), None);
        assert_eq!(
            read_limited_text(&b"abcd"[..], 4).unwrap().as_deref(),
            Some("abcd")
        );
        assert_eq!(read_limited_text(&b"abcde"[..], 4).unwrap(), None);
        assert!(read_limited_text(&[0xff][..], 4).is_err());
    }
}

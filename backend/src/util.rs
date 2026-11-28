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

/// Fast compression for local binary caches (zstd level 1 on Linux, raw fallback elsewhere).
pub fn compress_fast(data: &[u8]) -> (u8, Vec<u8>) {
    #[cfg(target_os = "linux")]
    if let Ok(compressed) = zstd::bulk::compress(data, 1)
        && compressed.len() < data.len()
    {
        return (1, compressed);
    }
    (0, data.to_vec())
}

/// Decompresses data produced by [`compress_fast`], rejecting unexpected output lengths.
pub fn decompress_fast(codec: u8, data: &[u8], expected_len: usize) -> Option<Vec<u8>> {
    match codec {
        0 if data.len() == expected_len => Some(data.to_vec()),
        #[cfg(target_os = "linux")]
        1 => zstd::bulk::decompress(data, expected_len)
            .ok()
            .filter(|out| out.len() == expected_len),
        _ => None,
    }
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

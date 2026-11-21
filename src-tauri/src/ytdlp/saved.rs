//! What a lookup printed (`yt-dlp -J`), kept so the download can start from
//! it (`--load-info-json`) instead of looking the video up all over again.
//!
//! Looking a YouTube video up takes about 2.8 s; starting from the saved
//! lookup, about 1 s, nearly all of it yt-dlp starting. The lookup holds the
//! stream links, and sites let those run out (YouTube after a few hours), so
//! it is used only while it is young (see [`REUSE_FOR`]). A try that fails is
//! retried with a fresh lookup (see `downloads::run`), so links that ran out
//! early cost one retry, not the download.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// How long a lookup is used for downloads. Most downloads start within
/// seconds of the lookup; links run out after hours on YouTube, but sooner
/// on some sites.
pub const REUSE_FOR: Duration = Duration::from_secs(15 * 60);

/// One lookup's JSON, as yt-dlp printed it.
#[derive(Debug)]
pub struct SavedLookup {
    at: Instant,
    /// Dropped once it is too old to use (see `MediaCache::insert`), so old
    /// lookups don't hold on to their hundred kilobytes or so.
    json: Mutex<Option<Vec<u8>>>,
}

impl SavedLookup {
    pub fn new(json: Vec<u8>) -> Self {
        Self {
            at: Instant::now(),
            json: Mutex::new(Some(json)),
        }
    }

    pub fn fresh(&self) -> bool {
        self.at.elapsed() < REUSE_FOR && self.json.lock().unwrap().is_some()
    }

    /// Lets go of the JSON once it is too old to use.
    pub fn drop_if_stale(&self) {
        if self.at.elapsed() >= REUSE_FOR {
            self.json.lock().unwrap().take();
        }
    }

    /// The lookup for yt-dlp to download from, if it is still young enough,
    /// with YouTube's `pieces` formats fetched in pieces (see [`in_pieces`]).
    pub fn for_download(&self, pieces: &[&str]) -> Option<Vec<u8>> {
        if self.at.elapsed() >= REUSE_FOR {
            return None;
        }
        let json = self.json.lock().unwrap().clone()?;
        if pieces.is_empty() {
            return Some(json);
        }
        let mut value: Value = serde_json::from_slice(&json).ok()?;
        in_pieces(&mut value, pieces);
        serde_json::to_vec(&value).ok()
    }
}

/// Makes YouTube's plain-file formats `ids` into the same files fetched in
/// pieces, the way yt-dlp's own `youtube:formats=dashy` does it (which only
/// applies while looking up): 10 MB ranges of the one link, asked for with
/// YouTube's `range` parameter. yt-dlp fetches a plain file on one connection,
/// whose speed varies a lot; pieces are fetched four at once
/// (`--concurrent-fragments`). See decisions.md, "Download speed".
///
/// Only a YouTube format that is a plain file of known size, with the request
/// size YouTube names, is changed; anything else is left as it was.
pub fn in_pieces(info: &mut Value, ids: &[&str]) {
    if info.get("extractor_key").and_then(Value::as_str) != Some("Youtube") {
        return;
    }
    let Some(formats) = info.get_mut("formats").and_then(Value::as_array_mut) else {
        return;
    };
    for format in formats.iter_mut().filter_map(Value::as_object_mut) {
        let id = format.get("format_id").and_then(Value::as_str).unwrap_or_default();
        if ids.contains(&id) {
            make_pieces(format);
        }
    }
}

fn make_pieces(format: &mut Map<String, Value>) {
    let plain = format.get("protocol").and_then(Value::as_str) == Some("https");
    let size = format.get("filesize").and_then(Value::as_u64).filter(|size| *size > 0);
    let chunk = format
        .get("downloader_options")
        .and_then(|options| options.get("http_chunk_size"))
        .and_then(Value::as_u64)
        .filter(|chunk| *chunk > 0);
    let url = format.get("url").and_then(Value::as_str);
    let (true, Some(size), Some(chunk), Some(url)) = (plain, size, chunk, url) else {
        return;
    };
    if url.contains("&range=") || url.contains("?range=") {
        return;
    }
    let join = if url.contains('?') { '&' } else { '?' };
    // As yt-dlp builds them, the last range included: it ends at the size,
    // one past the last byte, which YouTube answers with the bytes there are.
    let fragments: Vec<Value> = (0..size)
        .step_by(chunk as usize)
        .map(|start| {
            let end = (start + chunk - 1).min(size);
            serde_json::json!({ "url": format!("{url}{join}range={start}-{end}") })
        })
        .collect();
    format.insert("protocol".into(), "http_dash_segments".into());
    format.insert("fragments".into(), Value::Array(fragments));
    format.remove("downloader_options");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(extractor: &str) -> Value {
        serde_json::json!({
            "extractor_key": extractor,
            "formats": [
                {"format_id": "401", "protocol": "https", "filesize": 25_000_000_u64,
                 "url": "https://rr3.googlevideo.com/videoplayback?id=1&itag=401",
                 "downloader_options": {"http_chunk_size": 10_485_760_u64}},
                {"format_id": "251", "protocol": "https", "filesize": 3_000_000_u64,
                 "url": "https://rr3.googlevideo.com/videoplayback?id=1&itag=251",
                 "downloader_options": {"http_chunk_size": 10_485_760_u64}},
                {"format_id": "96", "protocol": "m3u8_native", "url": "https://manifest.googlevideo.com/x.m3u8"}
            ]
        })
    }

    #[test]
    fn makes_youtube_files_into_ten_megabyte_pieces_as_yt_dlp_does() {
        let mut info = lookup("Youtube");
        in_pieces(&mut info, &["401", "251", "96"]);
        let video = &info["formats"][0];
        assert_eq!(video["protocol"], "http_dash_segments");
        assert!(video.get("downloader_options").is_none());
        let ranges: Vec<&str> = video["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|piece| piece["url"].as_str().unwrap().rsplit("&range=").next().unwrap())
            .collect();
        // What yt-dlp's dashy wrote for the same sizes (seen 2026-11-21).
        assert_eq!(ranges, ["0-10485759", "10485760-20971519", "20971520-25000000"]);
        // A file smaller than a piece is one piece.
        assert_eq!(
            info["formats"][1]["fragments"][0]["url"]
                .as_str()
                .unwrap()
                .rsplit("&range=")
                .next(),
            Some("0-3000000")
        );
        // HLS is already in pieces.
        assert_eq!(info["formats"][2]["protocol"], "m3u8_native");
    }

    #[test]
    fn leaves_other_sites_and_formats_alone() {
        let original = lookup("Vimeo");
        let mut info = original.clone();
        in_pieces(&mut info, &["401", "251"]);
        assert_eq!(info, original);

        // Only the formats asked for.
        let mut info = lookup("Youtube");
        in_pieces(&mut info, &["251"]);
        assert_eq!(info["formats"][0]["protocol"], "https");

        // Without a size, or the request size YouTube names, there is nothing to split by.
        let mut info = lookup("Youtube");
        info["formats"][0].as_object_mut().unwrap().remove("filesize");
        info["formats"][1].as_object_mut().unwrap().remove("downloader_options");
        in_pieces(&mut info, &["401", "251"]);
        assert_eq!(info["formats"][0]["protocol"], "https");
        assert_eq!(info["formats"][1]["protocol"], "https");
    }

    #[test]
    fn is_used_only_while_young() {
        let saved = SavedLookup::new(b"{}".to_vec());
        assert!(saved.fresh());
        assert_eq!(saved.for_download(&[]).as_deref(), Some(&b"{}"[..]));
        // Right after Windows starts there is no instant that long ago to test with.
        let Some(then) = Instant::now().checked_sub(REUSE_FOR) else {
            return;
        };
        let old = SavedLookup {
            at: then,
            json: Mutex::new(Some(b"{}".to_vec())),
        };
        assert!(!old.fresh());
        assert_eq!(old.for_download(&[]), None);
        old.drop_if_stale();
        assert!(old.json.lock().unwrap().is_none());
    }
}

//! The bytes of a stream between two moments, read from the stream itself
//! rather than worked out from its average bitrate: a trimmed part of a
//! YouTube video came to between 0.56 and 1.65 times its share of the whole
//! (30 s parts of two 4K videos, 2026-11-22), since the busy scenes take more.
//!
//! - **A segmented stream** (HLS, Twitch's): the sizes of the segments the
//!   part covers, from the playlist where it states them, otherwise asked of
//!   the server (`HEAD`), a sample of them for a long part.
//! - **A file** (YouTube's streams): where in it the part starts and ends,
//!   read by ffprobe from the file's index and the few packets there.

use std::ffi::OsString;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use reqwest::Url;

use crate::process;
use crate::throttle::Flow;
use crate::ytdlp::metadata::Format;

/// How many segments' sizes are asked for at most; a longer part is scaled
/// from a sample of them, spread over it. A three-hour Twitch VOD's 1,128
/// segments came to within 0.6% of their total from 40 of them, in 0.7 s
/// (2026-11-22); asking is cheap, so three times that.
const MOST_SEGMENTS: usize = 120;
/// Asked at once.
const AT_ONCE: usize = 12;
/// How long ffprobe may take to read both ends of a part.
const PROBE_LIMIT: Duration = Duration::from_secs(30);
/// How long ffmpeg and ffprobe wait on a connection that sends nothing, in microseconds.
pub const READ_TIMEOUT_US: &str = "20000000";

/// The bytes of `format` from `start` to `end` seconds, `None` if they can't be read.
pub async fn bytes(
    ffprobe: &std::path::Path,
    flow: &Flow,
    format: &Format,
    start: f64,
    end: f64,
    duration: f64,
) -> Option<u64> {
    let url = format.stream_url.as_deref()?;
    if format.segmented {
        segmented(format, url, start, end).await
    } else {
        file(ffprobe, flow, format, url, start, end, duration).await
    }
}

/// A segment of a media playlist.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    /// Seconds from the start of the stream.
    start: f64,
    duration: f64,
    url: String,
    /// Its size, where the playlist says (`#EXT-X-BYTERANGE`).
    size: Option<u64>,
}

/// The segments of an HLS media playlist, with their links made whole
/// against the playlist's own. Empty for a master playlist, or one that
/// can't be read.
fn segments(playlist: &str, base: &Url) -> Vec<Segment> {
    if playlist.contains("#EXT-X-STREAM-INF") {
        return Vec::new();
    }
    let mut segments = Vec::new();
    let mut at = 0.0;
    let mut duration = None;
    let mut size = None;
    for line in playlist.lines().map(str::trim) {
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            duration = info
                .split(',')
                .next()
                .and_then(|seconds| seconds.trim().parse::<f64>().ok());
        } else if let Some(range) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            size = range.split('@').next().and_then(|length| length.trim().parse().ok());
        } else if !line.is_empty() && !line.starts_with('#') {
            if let (Some(length), Ok(url)) = (duration.take(), base.join(line)) {
                segments.push(Segment {
                    start: at,
                    duration: length,
                    url: url.to_string(),
                    size: size.take(),
                });
                at += length;
            }
        }
    }
    segments
}

/// How much of `segment` lies between `start` and `end`, 0–1.
fn overlap(segment: &Segment, start: f64, end: f64) -> f64 {
    if segment.duration <= 0.0 {
        return 0.0;
    }
    let from = segment.start.max(start);
    let to = (segment.start + segment.duration).min(end);
    ((to - from) / segment.duration).clamp(0.0, 1.0)
}

/// The segments to ask the size of: all of them, or [`MOST_SEGMENTS`] spread evenly.
fn sample(count: usize) -> Vec<usize> {
    if count <= MOST_SEGMENTS {
        return (0..count).collect();
    }
    (0..MOST_SEGMENTS)
        .map(|i| (i * 2 + 1) * count / (MOST_SEGMENTS * 2))
        .collect()
}

/// The bytes of the segments between `start` and `end`: their stated or
/// asked sizes, each counted for the share of it inside, and a sample's
/// bytes per second over the rest when there are many.
fn sum(segments: &[Segment], sizes: &[(usize, u64)], start: f64, end: f64) -> Option<u64> {
    let seconds = |index: usize| segments[index].duration * overlap(&segments[index], start, end);
    let covered: f64 = sizes.iter().map(|(index, _)| seconds(*index)).sum();
    let bytes: f64 = sizes
        .iter()
        .map(|(index, size)| *size as f64 * overlap(&segments[*index], start, end))
        .sum();
    let total: f64 = (0..segments.len()).map(seconds).sum();
    // At least most of the segments asked must have answered.
    let asked = sample(segments.len()).len();
    if covered <= 0.0 || sizes.len() * 4 < asked * 3 {
        return None;
    }
    Some((bytes / covered * total) as u64)
}

async fn segmented(format: &Format, url: &str, start: f64, end: f64) -> Option<u64> {
    let base = Url::parse(url).ok()?;
    let http = crate::throttle::follow_system_proxy(reqwest::Client::builder())
        .timeout(Duration::from_secs(20))
        .build()
        .ok()?;
    let request = |method: reqwest::Method, link: &str| {
        let mut request = http.request(method, link);
        for (name, value) in &format.http_headers {
            request = request.header(name, value);
        }
        request
    };
    let playlist = request(reqwest::Method::GET, url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    let all = segments(&playlist, &base);
    let inside: Vec<Segment> = all
        .into_iter()
        .filter(|segment| overlap(segment, start, end) > 0.0)
        .collect();
    if inside.is_empty() {
        return None;
    }
    let sizes: Vec<(usize, u64)> = stream::iter(sample(inside.len()))
        .map(|index| {
            let segment = &inside[index];
            let head = request(reqwest::Method::HEAD, &segment.url);
            async move {
                if let Some(size) = segment.size {
                    return Some((index, size));
                }
                let response = head.send().await.ok()?.error_for_status().ok()?;
                response
                    .content_length()
                    .filter(|size| *size > 0)
                    .map(|size| (index, size))
            }
        })
        .buffer_unordered(AT_ONCE)
        .filter_map(|size| async move { size })
        .collect()
        .await;
    sum(&inside, &sizes, start, end)
}

/// The arguments that make ffprobe print where in the file the packets
/// around `start` and `end` lie: their times and byte positions.
fn probe_args(format: &Format, url: &str, start: f64, end: f64) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-v", "error", "-rw_timeout", READ_TIMEOUT_US]
        .map(OsString::from)
        .to_vec();
    args.extend(header_args(format));
    // The packets from the key frame before each moment to just past it.
    let intervals = format!("{start:.3}%{:.3},{end:.3}%{:.3}", start + 0.5, end + 0.5);
    args.extend(
        [
            "-select_streams",
            "0",
            "-show_entries",
            "packet=pts_time,pos",
            "-of",
            "csv=p=0",
            "-read_intervals",
            &intervals,
        ]
        .map(OsString::from),
    );
    args.push(url.into());
    args
}

/// The headers yt-dlp sends with the stream's requests, as ffmpeg and ffprobe take them.
pub fn header_args(format: &Format) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut others = String::new();
    for (name, value) in &format.http_headers {
        if name.eq_ignore_ascii_case("user-agent") {
            args.extend(["-user_agent".into(), value.into()]);
        } else if !value.contains(['\r', '\n']) {
            others.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if !others.is_empty() {
        args.extend(["-headers".into(), others.into()]);
    }
    args
}

/// Where the part from `start` to `end` lies in the file, from what
/// [`probe_args`] printed: the position of the first packet at or after
/// each moment, or the end of the file for a part that runs to it.
fn span_in_file(printed: &str, start: f64, end: f64, file_size: Option<u64>) -> Option<u64> {
    let packets: Vec<(f64, u64)> = printed
        .lines()
        .filter_map(|line| {
            let (time, pos) = line.trim().split_once(',')?;
            Some((time.parse().ok()?, pos.trim_end_matches(',').parse().ok()?))
        })
        .collect();
    let at = |moment: f64| {
        packets
            .iter()
            .find(|(time, _)| *time >= moment - 0.0005)
            .map(|(_, pos)| *pos)
    };
    let from = at(start)?;
    let to = file_size.or_else(|| at(end))?;
    to.checked_sub(from).filter(|bytes| *bytes > 0)
}

async fn file(
    ffprobe: &std::path::Path,
    flow: &Flow,
    format: &Format,
    url: &str,
    start: f64,
    end: f64,
    duration: f64,
) -> Option<u64> {
    let mut cmd = process::command(ffprobe);
    cmd.args(probe_args(format, url, start, end));
    flow.route_ffmpeg(&mut cmd);
    let output = process::output_within(cmd, PROBE_LIMIT).await.ok()??;
    // A part that runs to the end ends where the file does, when its size is stated.
    let runs_out = end >= duration - 0.25;
    let file_size = format.filesize.filter(|_| runs_out && !format.filesize_estimated);
    span_in_file(&String::from_utf8_lossy(&output.stdout), start, end, file_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, duration: f64) -> Segment {
        Segment {
            start,
            duration,
            url: String::new(),
            size: None,
        }
    }

    #[test]
    fn reads_a_vod_playlist() {
        // Twitch's, as served 2026-11-22: fragmented MP4 after its initialisation segment.
        let playlist = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:10\n#EXT-X-PLAYLIST-TYPE:EVENT\n\
            #EXT-X-MAP:URI=\"init-0.mp4\"\n#EXTINF:10.000,\n0.mp4\n#EXTINF:10.000,\n1.mp4\n#EXTINF:4.200,\n2-muted.mp4\n";
        let base = Url::parse("https://d3fi1amfgojobc.cloudfront.net/abc/1080p60/index-dvr.m3u8").unwrap();
        let read = segments(playlist, &base);
        assert_eq!(read.len(), 3);
        assert_eq!(read[1].start, 10.0);
        assert_eq!(read[2].duration, 4.2);
        assert_eq!(
            read[2].url,
            "https://d3fi1amfgojobc.cloudfront.net/abc/1080p60/2-muted.mp4"
        );
        assert_eq!(read[0].size, None);
        // Byte ranges say the size; a master playlist has no segments of its own.
        let ranged = segments("#EXTINF:6.0,\n#EXT-X-BYTERANGE:120000@0\nall.ts\n", &base);
        assert_eq!(ranged[0].size, Some(120_000));
        assert!(segments("#EXT-X-STREAM-INF:BANDWIDTH=1\nlow.m3u8\n", &base).is_empty());
    }

    #[test]
    fn counts_each_segment_for_its_share_of_the_part() {
        let segments = [segment(0.0, 10.0), segment(10.0, 10.0), segment(20.0, 10.0)];
        // 5–25 s: half of the first, all of the second, half of the third.
        let sizes = [(0, 1000), (1, 3000), (2, 2000)];
        assert_eq!(sum(&segments, &sizes, 5.0, 25.0), Some(500 + 3000 + 1000));
        assert_eq!(overlap(&segments[0], 20.0, 30.0), 0.0);
        // Too few answers to go by.
        assert_eq!(sum(&segments, &sizes[..1], 5.0, 25.0), None);
    }

    #[test]
    fn scales_a_long_part_from_a_spread_sample() {
        let segments: Vec<Segment> = (0..400).map(|i| segment(f64::from(i) * 10.0, 10.0)).collect();
        let picked = sample(segments.len());
        assert_eq!(picked.len(), MOST_SEGMENTS);
        assert_eq!((picked[0], *picked.last().unwrap()), (1, 398));
        // Each picked segment holds a megabyte: the whole stream, 400 of them.
        let sizes: Vec<(usize, u64)> = picked.iter().map(|index| (*index, 1_000_000)).collect();
        assert_eq!(sum(&segments, &sizes, 0.0, 4000.0), Some(400_000_000));
    }

    #[test]
    fn finds_where_a_part_lies_in_a_file() {
        // ffprobe from the key frame before each moment to just past it.
        let printed = "118.952000,40000000\n119.786000,40400000\n120.019000,40500000\n120.453000,40600000\n\
            179.947000,66000000\n180.014000,66100000\n180.481000,66200000\n";
        assert_eq!(span_in_file(printed, 120.0, 180.0, None), Some(25_600_000));
        // A part that runs to the end of a file whose size is stated.
        assert_eq!(
            span_in_file(printed, 120.0, 635.0, Some(712_445_280)),
            Some(671_945_280)
        );
        assert_eq!(span_in_file("N/A,N/A\n", 120.0, 180.0, None), None);
    }

    #[test]
    fn hands_the_sites_headers_to_ffmpeg() {
        let format = Format {
            http_headers: vec![
                ("User-Agent".into(), "Mozilla/5.0".into()),
                ("Referer".into(), "https://example.com/".into()),
                ("Bad".into(), "a\r\nInjected: yes".into()),
            ],
            ..super::super::test_format("1")
        };
        let args: Vec<String> = header_args(&format)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-user_agent",
                "Mozilla/5.0",
                "-headers",
                "Referer: https://example.com/\r\n"
            ]
        );
    }
}

//! Machine-readable progress from yt-dlp.
//!
//! We give yt-dlp templates that print one tagged, pipe-separated line per
//! update, then parse those lines here. Missing values arrive as "NA".

/// Args that make yt-dlp report progress in the format [`parse`] understands.
pub fn template_args() -> [&'static str; 15] {
    [
        // One line per update instead of redrawing with carriage returns.
        "--newline",
        // `--print` implies quiet + simulate; undo both.
        "--progress",
        "--no-simulate",
        // Ten updates a second is plenty for a smooth bar.
        "--progress-delta",
        "0.1",
        "--progress-template",
        // No speed or time left: yt-dlp's are readings over a moment, so
        // Siphon measures its own from the byte counts (see `downloads::tracker`).
        "download:SIPHON_DL|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(info.format_id)s|%(info.vcodec)s|%(info.acodec)s|%(progress.filename)s",
        "--progress-template",
        "postprocess:SIPHON_PP|%(progress.status)s|%(progress.postprocessor)s",
        // Which streams were picked and how big they are, before any bytes move.
        "--print",
        "before_dl:SIPHON_PLAN|%(filesize,filesize_approx)s|%(requested_formats.:.{format_id,filesize,filesize_approx})j",
        // Where the download is being written (its ".part" file can be watched).
        "--print",
        "before_dl:SIPHON_TARGET|%(filename)s",
        // Where the finished file ended up.
        "--print",
        "after_move:SIPHON_FILE|%(filepath)s",
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// The streams about to be downloaded, with their sizes in bytes when known.
    Plan(Vec<PlannedStream>),
    Download(DownloadTick),
    PostProcess {
        started: bool,
        name: String,
    },
    /// Path the download will be written to.
    Target(String),
    /// Final path of the finished file.
    File(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedStream {
    pub format_id: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadTick {
    /// False for the final "finished" tick of a stream.
    pub downloading: bool,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    pub format_id: String,
    pub has_video: bool,
    pub has_audio: bool,
    /// The file this stream is being written to (before any merge).
    pub filename: Option<String>,
}

/// Parses one stdout line. Returns `None` for anything that isn't ours.
pub fn parse(line: &str) -> Option<Line> {
    let line = line.trim();
    // Fetching several streams at once (a live stream from its start), yt-dlp
    // puts the stream's number in front of each line: "1: SIPHON_DL|…".
    let line = match line.split_once(": ") {
        Some((number, rest)) if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => line,
    };
    if let Some(rest) = line.strip_prefix("SIPHON_DL|") {
        // The file name goes last because it may itself contain pipes.
        let fields: Vec<&str> = rest.splitn(8, '|').collect();
        let [status, downloaded, total, estimate, format_id, vcodec, acodec, filename] = fields[..] else {
            return None;
        };
        return Some(Line::Download(DownloadTick {
            downloading: status != "finished",
            downloaded: bytes(downloaded),
            total: bytes(total).or(bytes(estimate)),
            format_id: format_id.to_string(),
            has_video: !matches!(vcodec, "none" | "NA"),
            has_audio: !matches!(acodec, "none" | "NA"),
            filename: Some(filename).filter(|name| *name != "NA").map(str::to_string),
        }));
    }
    if let Some(rest) = line.strip_prefix("SIPHON_PP|") {
        let (status, name) = rest.split_once('|')?;
        return Some(Line::PostProcess {
            started: status == "started",
            name: name.to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix("SIPHON_PLAN|") {
        // The JSON goes last because it may itself contain pipes.
        let (single_size, streams) = rest.split_once('|')?;
        return Some(Line::Plan(plan(single_size, streams)));
    }
    if let Some(path) = line.strip_prefix("SIPHON_TARGET|") {
        return Some(Line::Target(path.to_string()));
    }
    line.strip_prefix("SIPHON_FILE|")
        .map(|path| Line::File(path.to_string()))
}

fn plan(single_size: &str, streams_json: &str) -> Vec<PlannedStream> {
    #[derive(serde::Deserialize)]
    struct Stream {
        format_id: Option<String>,
        filesize: Option<f64>,
        filesize_approx: Option<f64>,
    }
    match serde_json::from_str::<Vec<Stream>>(streams_json) {
        // Separate video + audio streams that get merged afterwards.
        Ok(streams) if !streams.is_empty() => streams
            .into_iter()
            .map(|stream| PlannedStream {
                format_id: stream.format_id.unwrap_or_default(),
                size: stream.filesize.or(stream.filesize_approx).map(|n| n as u64),
            })
            .collect(),
        // A single pre-muxed file.
        _ => vec![PlannedStream {
            format_id: String::new(),
            size: bytes(single_size),
        }],
    }
}

fn float(field: &str) -> Option<f64> {
    field.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn bytes(field: &str) -> Option<u64> {
    float(field).filter(|n| *n >= 0.0).map(|n| n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_download_tick() {
        let line = r"SIPHON_DL|downloading|1048576|10485760.0|NA|137|avc1.640028|none|C:\v\a|b.f137.mp4";
        let Some(Line::Download(tick)) = parse(line) else {
            panic!("not a tick")
        };
        assert!(tick.downloading);
        assert_eq!(tick.filename.as_deref(), Some(r"C:\v\a|b.f137.mp4"));
        assert_eq!(tick.downloaded, Some(1_048_576));
        assert_eq!(tick.total, Some(10_485_760));
        assert_eq!(tick.format_id, "137");
        assert!(tick.has_video && !tick.has_audio);
    }

    #[test]
    fn falls_back_to_the_size_estimate() {
        let line = "SIPHON_DL|downloading|10|NA|2000.7|251|none|opus|a.webm";
        let Some(Line::Download(tick)) = parse(line) else {
            panic!("not a tick")
        };
        assert_eq!(tick.total, Some(2000));
        assert!(!tick.has_video && tick.has_audio);
    }

    #[test]
    fn parses_a_two_stream_plan() {
        let line =
            r#"SIPHON_PLAN|NA|[{"format_id": "137", "filesize": 900}, {"format_id": "251", "filesize_approx": 100.5}]"#;
        assert_eq!(
            parse(line),
            Some(Line::Plan(vec![
                PlannedStream {
                    format_id: "137".into(),
                    size: Some(900)
                },
                PlannedStream {
                    format_id: "251".into(),
                    size: Some(100)
                },
            ]))
        );
    }

    #[test]
    fn parses_a_single_file_plan() {
        assert_eq!(
            parse("SIPHON_PLAN|4096|NA"),
            Some(Line::Plan(vec![PlannedStream {
                format_id: String::new(),
                size: Some(4096)
            }]))
        );
    }

    #[test]
    fn keeps_pipes_in_file_names() {
        assert_eq!(
            parse(r"SIPHON_FILE|C:\Videos\a|b.mp4"),
            Some(Line::File(r"C:\Videos\a|b.mp4".into()))
        );
    }

    #[test]
    fn ignores_ordinary_output() {
        assert_eq!(parse("[youtube] Extracting URL"), None);
    }

    #[test]
    fn reads_the_lines_of_streams_fetched_side_by_side() {
        // A live stream from the start (2026-11-19): video and audio in two threads.
        let line = r"2: SIPHON_DL|downloading|81923|NA|NA|140|none|mp4a.40.2|C:\rec\Show.siphon-ab.rec1.f140.mkv";
        let Some(Line::Download(tick)) = parse(line) else {
            panic!("not read: {line}");
        };
        assert_eq!(tick.format_id, "140");
        assert_eq!(tick.filename.as_deref(), Some(r"C:\rec\Show.siphon-ab.rec1.f140.mkv"));
        // A title that starts with a number and a colon is no thread number.
        assert!(parse("1: A song: SIPHON_DL|x").is_none());
    }
}

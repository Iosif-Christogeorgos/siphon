//! What a picture subtitle track (a Blu-ray's PGS, a DVD's VobSub) says, read
//! with Windows' own OCR (`Windows.Media.Ocr`, part of Windows): a dozen of
//! its pictures, spread across the stretches read for text subtitles, are
//! drawn by FFmpeg dark on white, without the video, and read by every
//! reader Windows has, one for each language installed on the PC.
//!
//! A reading may bear a tag out, or name the language of an untagged track;
//! it never overrules a tag. A reader shown a script it doesn't know makes
//! words of it all the same (Greek letters of Cyrillic), so a reading that
//! disagrees with the tag only casts doubt on it.

use std::ffi::OsString;
use std::path::Path;

use serde_json::Value;

use super::words::Reading;
use super::Track;
use crate::ytdlp::subtitles::{language_name, primary};

/// Subtitles made of pictures, by ffprobe's codec names.
pub const PICTURES: &[&str] = &["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle"];

/// Pictures read from a track, at most.
const MOST: usize = 12;
/// A packet this small clears the screen rather than showing a line.
const SMALLEST: u64 = 400;

/// The arguments that make ffprobe list the packets of the track at `index`:
/// when each starts, and how large it is.
pub fn packet_args(file: &Path, index: usize, windows: Option<&[(f64, f64)]>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-v", "error"].map(OsString::from).to_vec();
    if let Some(windows) = windows {
        let intervals: Vec<String> = windows
            .iter()
            .map(|(start, seconds)| format!("{start:.3}%+{seconds:.3}"))
            .collect();
        args.extend(["-read_intervals".into(), intervals.join(",").into()]);
    }
    args.extend(
        [
            "-select_streams",
            &index.to_string(),
            "-show_entries",
            "packet=pts_time,size",
            "-print_format",
            "json",
        ]
        .map(OsString::from),
    );
    args.push(file.into());
    args
}

/// When the pictures to read show, from what [`packet_args`] printed: at most
/// [`MOST`], spread evenly over those that show a line; and how many show one.
pub fn moments(json: &str) -> (Vec<f64>, usize) {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return (Vec::new(), 0);
    };
    let packets = value["packets"].as_array().map(Vec::as_slice).unwrap_or_default();
    let shown: Vec<f64> = packets
        .iter()
        .filter(|packet| number(&packet["size"]).is_some_and(|size| size >= SMALLEST as f64))
        .filter_map(|packet| number(&packet["pts_time"]))
        .collect();
    let step = (shown.len() as f64 / MOST as f64).max(1.0);
    let picked = (0..MOST.min(shown.len()))
        .map(|i| shown[(i as f64 * step + step / 2.0) as usize % shown.len()])
        .collect();
    (picked, shown.len())
}

/// The arguments that make FFmpeg draw the picture showing at `at` seconds,
/// dark on white, `width` by `height`, as 8-bit grey on its output.
pub fn draw_args(file: &Path, index: usize, at: f64, width: u32, height: u32) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-ss"]
        .map(OsString::from)
        .to_vec();
    // From a little before, so the packet is read; the picture is taken just after it shows.
    args.push(format!("{:.3}", (at - 0.5).max(0.0)).into());
    args.extend(["-i".into(), file.into(), "-filter_complex".into()]);
    args.push(
        format!(
            "[0:{index}]negate[lines];color=c=white:s={width}x{height}:r=10[page];\
             [page][lines]overlay=eof_action=pass,format=gray"
        )
        .into(),
    );
    args.extend(
        [
            "-ss",
            "0.6",
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ]
        .map(OsString::from),
    );
    args
}

/// What a picture subtitle track's readings say about it: the reading of
/// each reader Windows has, one per language installed.
pub fn apply(track: &mut Track, readings: &[Reading]) {
    let mut read: Vec<&str> = readings
        .iter()
        .filter_map(|reading| reading.language.as_deref())
        .collect();
    read.sort_unstable();
    read.dedup();
    match (track.language.clone(), read.as_slice()) {
        (Some(tagged), read) if read.iter().any(|language| primary(language) == primary(&tagged)) => {
            track.read = true;
        }
        (None, [language]) => {
            track.language = Some((*language).to_string());
            track.read = true;
        }
        (Some(tagged), [language, ..]) => track.doubts.push(format!(
            "Windows read its pictures as {}, but it is tagged {}.",
            name(language),
            name(&tagged)
        )),
        (Some(_), []) => track
            .notes
            .push("Windows couldn't read its pictures in a language it knows, so its tag is taken.".to_string()),
        (None, _) => {}
    }
    // The fullest reading tells SDH, as for text.
    if let Some(fullest) = readings.iter().max_by_key(|reading| reading.lines) {
        if fullest.sdh() && !track.sdh {
            track.sdh = true;
            let example = fullest
                .sound
                .as_deref()
                .map(|line| format!(": \"{line}\""))
                .unwrap_or_default();
            track.notes.push(format!(
                "Its lines describe sounds, as subtitles for the deaf do{example}."
            ));
        }
    }
}

fn name(tag: &str) -> String {
    language_name(tag).map_or_else(|| tag.to_string(), str::to_string)
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// Windows' OCR readers, one for each language installed on the PC.
pub struct Readers(Vec<windows::Media::Ocr::OcrEngine>);

impl Readers {
    /// Every reader Windows has. None if it has none, or OCR can't be had.
    pub fn new() -> Option<Readers> {
        use windows::Media::Ocr::OcrEngine;
        let languages = OcrEngine::AvailableRecognizerLanguages().ok()?;
        let readers: Vec<OcrEngine> = languages
            .into_iter()
            .filter_map(|language| OcrEngine::TryCreateFromLanguage(&language).ok())
            .collect();
        (!readers.is_empty()).then_some(Readers(readers))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The lines each reader finds in a picture of `gray` bytes, `width` by
    /// `height`, one list for each reader. Blocks while they read.
    pub fn read(&self, gray: &[u8], width: u32, height: u32) -> Vec<Vec<String>> {
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Security::Cryptography::CryptographicBuffer;
        let bitmap = CryptographicBuffer::CreateFromByteArray(gray).and_then(|buffer| {
            SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Gray8, width as i32, height as i32)
        });
        let Ok(bitmap) = bitmap else {
            return vec![Vec::new(); self.0.len()];
        };
        self.0
            .iter()
            .map(|reader| {
                let Ok(result) = reader.RecognizeAsync(&bitmap).and_then(|reading| reading.join()) else {
                    return Vec::new();
                };
                let lines = result
                    .Lines()
                    .map(|lines| lines.into_iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                lines
                    .iter()
                    .filter_map(|line| line.Text().ok())
                    .map(|text| text.to_string())
                    .filter(|text| !text.trim().is_empty())
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::mkv::Kind;

    fn track(language: Option<&str>) -> Track {
        Track {
            index: 5,
            kind: Kind::Subtitles,
            codec: "hdmv_pgs_subtitle".into(),
            name: None,
            language: language.map(str::to_string),
            default: false,
            forced: false,
            sdh: false,
            commentary: false,
            described: false,
            original: false,
            channels: None,
            width: Some(1920),
            height: Some(1080),
            entries: None,
            bytes: None,
            doubts: Vec::new(),
            notes: Vec::new(),
            read: false,
            certain: false,
        }
    }

    fn reading(language: Option<&str>) -> Reading {
        Reading {
            language: language.map(str::to_string),
            lines: 12,
            ..Reading::default()
        }
    }

    #[test]
    fn a_reading_bears_a_tag_out_or_names_an_untagged_track() {
        // The Greek reader reads English too; the English one makes nonsense of Greek.
        let mut english = track(Some("en"));
        apply(&mut english, &[reading(Some("en")), reading(Some("en"))]);
        assert!(english.read && english.doubts.is_empty());

        let mut greek = track(Some("el"));
        apply(&mut greek, &[reading(Some("el")), reading(None)]);
        assert!(greek.read);

        let mut untagged = track(None);
        apply(&mut untagged, &[reading(Some("el")), reading(None)]);
        assert_eq!((untagged.language.as_deref(), untagged.read), (Some("el"), true));

        // Two readers saying two languages name nothing.
        let mut torn = track(None);
        apply(&mut torn, &[reading(Some("el")), reading(Some("hu"))]);
        assert_eq!(torn.language, None);
    }

    #[test]
    fn a_reading_never_overrules_a_tag() {
        // Russian, read by a Greek reader that made Greek letters of it.
        let mut russian = track(Some("ru"));
        apply(&mut russian, &[reading(Some("el")), reading(None)]);
        assert_eq!(russian.language.as_deref(), Some("ru"));
        assert_eq!(
            russian.doubts,
            ["Windows read its pictures as Greek, but it is tagged Russian."]
        );

        let mut unread = track(Some("ar"));
        apply(&mut unread, &[reading(None), reading(None)]);
        assert!(!unread.read && unread.doubts.is_empty());
        assert_eq!(
            unread.notes,
            ["Windows couldn't read its pictures in a language it knows, so its tag is taken."]
        );
    }

    #[test]
    fn picks_pictures_that_show_lines_spread_over_the_track() {
        let packets: Vec<Value> = (0..100)
            .map(|i| serde_json::json!({ "pts_time": format!("{}.000000", i * 2), "size": if i % 2 == 0 { "5000" } else { "30" } }))
            .collect();
        let (picked, shown) = moments(&serde_json::json!({ "packets": packets }).to_string());
        assert_eq!(shown, 50);
        assert_eq!(picked.len(), MOST);
        assert!(picked.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(picked.iter().all(|at| (*at as i64) % 4 == 0));
        assert_eq!(moments(r#"{"packets": []}"#), (Vec::new(), 0));
    }
}

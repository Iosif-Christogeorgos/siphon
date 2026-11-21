//! SubRip subtitles: reading, cleaning, cutting to a clip, and writing.
//!
//! Every subtitle track Siphon adds goes through here as SubRip (yt-dlp turns
//! WebVTT and TTML into it; see `ytdlp::args::subtitle_args`), except styled
//! SubStation Alpha kept whole in Matroska. What comes out:
//!
//! - **No overlaps.** YouTube's speech-recognition captions overlap, each line
//!   staying up while the next arrives, so two show at once, rolling up
//!   (measured 2026-11-20: "We're no strangers to" from 18.8 to 25.96 s,
//!   "love. You know the rules…" from 21.8 s). An MP4 subtitle track can't
//!   hold cues that overlap, so the times are split where any cue starts or
//!   ends, each piece showing every line up then: what a player showing the
//!   overlapping cues would show.
//! - **No empty cues**, and none of zero length.
//! - **Cut to a clip**, on the video's own clock: the ffmpeg pass shifts them
//!   with the picture.

use std::fmt::Write as _;

/// One subtitle: shown from `start` to `end`, in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start: u64,
    pub end: u64,
    pub text: String,
}

/// The cues of a SubRip file, in the order written. Lenient: a missing
/// number, `.` for `,`, minutes without hours, Windows line ends, a byte-order
/// mark and WebVTT's settings after the times are all read past; a block
/// without times is skipped.
pub fn parse(text: &str) -> Vec<Cue> {
    let text = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|line| !line.contains("-->"));
        let Some(times) = lines.next() else { continue };
        let Some((start, end)) = times.split_once("-->") else {
            continue;
        };
        let end = end.split_whitespace().next().unwrap_or_default();
        let (Some(start), Some(end)) = (clock(start.trim()), clock(end)) else {
            continue;
        };
        let text = lines.map(str::trim_end).collect::<Vec<_>>().join("\n");
        cues.push(Cue {
            start,
            end,
            text: text.trim().to_string(),
        });
    }
    cues
}

/// "01:02:03,456", "01:02:03.456" or "02:03.456" → milliseconds.
fn clock(time: &str) -> Option<u64> {
    let mut parts: Vec<&str> = time.split(':').collect();
    let seconds = parts.pop()?.replace(',', ".");
    let (whole, fraction) = seconds.split_once('.').unwrap_or((&seconds, "0"));
    let mut ms: u64 = whole.trim().parse::<u64>().ok()? * 1000;
    // "5" is 500 ms, "05" is 50, "005" is 5.
    let digits: String = fraction.chars().take(3).collect();
    if !digits.is_empty() {
        ms += digits.parse::<u64>().ok()? * 10u64.pow(3 - digits.len() as u32);
    }
    let mut scale = 60_000;
    for part in parts.iter().rev() {
        ms += part.trim().parse::<u64>().ok()? * scale;
        scale *= 60;
    }
    Some(ms)
}

/// Writes cues as a SubRip file, numbered from one.
pub fn write(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            stamp(cue.start),
            stamp(cue.end),
            cue.text
        );
    }
    out
}

fn stamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// The part of the video subtitles are wanted for, in seconds on the video's
/// own clock (the ffmpeg pass shifts the track with the picture).
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

/// Cleans `cues`, cuts them to `span` if any, and splits overlaps: see the
/// module's notes. Empty when nothing is left.
pub fn prepare(cues: Vec<Cue>, span: Option<Span>) -> Vec<Cue> {
    let mut cues: Vec<Cue> = cues
        .into_iter()
        .filter(|cue| cue.end > cue.start && !cue.text.trim().is_empty())
        .collect();
    if let Some(span) = span {
        let (from, to) = ((span.start * 1000.0).round() as u64, (span.end * 1000.0).round() as u64);
        cues = cues
            .into_iter()
            .filter(|cue| cue.end > from && cue.start < to)
            .map(|cue| Cue {
                start: cue.start.max(from),
                end: cue.end.min(to),
                text: cue.text,
            })
            .filter(|cue| cue.end > cue.start)
            .collect();
    }
    flatten(cues)
}

/// Splits overlapping cues where any starts or ends, each piece showing the
/// lines of every cue up then, oldest first; the same line twice once.
fn flatten(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.sort_by_key(|cue| (cue.start, cue.end));
    let mut times: Vec<u64> = cues.iter().flat_map(|cue| [cue.start, cue.end]).collect();
    times.sort_unstable();
    times.dedup();

    let mut out: Vec<Cue> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    for window in times.windows(2) {
        let (from, to) = (window[0], window[1]);
        active.retain(|&index| cues[index].end > from);
        while next < cues.len() && cues[next].start <= from {
            if cues[next].end > from {
                active.push(next);
            }
            next += 1;
        }
        let mut lines: Vec<&str> = Vec::new();
        for &index in &active {
            for line in cues[index].text.lines() {
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
        if lines.is_empty() {
            continue;
        }
        let text = lines.join("\n");
        match out.last_mut() {
            // The same lines straight on: one cue, not two.
            Some(last) if last.end == from && last.text == text => last.end = to,
            _ => out.push(Cue {
                start: from,
                end: to,
                text,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: u64, end: u64, text: &str) -> Cue {
        Cue {
            start,
            end,
            text: text.into(),
        }
    }

    #[test]
    fn reads_subrip_however_it_is_written() {
        let file = "\u{feff}1\r\n00:00:01,200 --> 00:00:03,360\r\nAll right, so here we are,\r\nin front of the elephants\r\n\r\n\
                    2\r\n00:00:05.318 --> 00:00:07.974 align:start position:0%\r\nthe cool thing\r\n\r\n\
                    00:01.5 --> 00:02.25\r\nno number\r\n\r\nnot a cue at all\r\n";
        assert_eq!(
            parse(file),
            [
                cue(1200, 3360, "All right, so here we are,\nin front of the elephants"),
                cue(5318, 7974, "the cool thing"),
                cue(1500, 2250, "no number"),
            ]
        );
        assert_eq!(clock("1:00:00,000"), Some(3_600_000));
        assert_eq!(clock("nonsense"), None);
    }

    #[test]
    fn writes_what_it_reads() {
        let cues = vec![cue(1200, 3360, "One\nTwo"), cue(3_723_004, 3_724_000, "Later")];
        let text = write(&cues);
        assert_eq!(
            text,
            "1\n00:00:01,200 --> 00:00:03,360\nOne\nTwo\n\n2\n01:02:03,004 --> 01:02:04,000\nLater\n\n"
        );
        assert_eq!(parse(&text), cues);
    }

    #[test]
    fn rolling_captions_become_cues_that_never_overlap() {
        // YouTube's speech recognition, as its SubRip came on 2026-11-20.
        let rolling = vec![
            cue(320, 14_580, "[Music]"),
            cue(18_800, 25_960, "We're no strangers to"),
            cue(21_800, 29_119, "love. You know the rules and so do"),
            cue(25_960, 30_279, "I. I feel commitments"),
        ];
        assert_eq!(
            prepare(rolling, None),
            [
                cue(320, 14_580, "[Music]"),
                cue(18_800, 21_800, "We're no strangers to"),
                cue(
                    21_800,
                    25_960,
                    "We're no strangers to\nlove. You know the rules and so do"
                ),
                cue(
                    25_960,
                    29_119,
                    "love. You know the rules and so do\nI. I feel commitments"
                ),
                cue(29_119, 30_279, "I. I feel commitments"),
            ]
        );
        // The same line twice shows once, and runs on as one cue.
        assert_eq!(
            prepare(
                vec![cue(0, 2000, "Hi"), cue(1000, 3000, "Hi"), cue(3000, 4000, "Hi")],
                None
            ),
            [cue(0, 4000, "Hi")]
        );
    }

    #[test]
    fn empty_and_backwards_cues_are_dropped() {
        let cues = vec![
            cue(0, 1000, "  "),
            cue(2000, 2000, "zero"),
            cue(5000, 4000, "backwards"),
            cue(1, 2, "ok"),
        ];
        assert_eq!(prepare(cues, None), [cue(1, 2, "ok")]);
    }

    #[test]
    fn a_clip_keeps_what_shows_inside_it() {
        let cues = vec![
            cue(1000, 4000, "before and into"),
            cue(5000, 6000, "inside"),
            cue(9000, 12_000, "out of"),
            cue(20_000, 21_000, "after"),
        ];
        let span = Some(Span { start: 3.0, end: 10.0 });
        // On the video's clock, for the ffmpeg pass to shift with the picture.
        assert_eq!(
            prepare(cues, span),
            [
                cue(3000, 4000, "before and into"),
                cue(5000, 6000, "inside"),
                cue(9000, 10_000, "out of")
            ]
        );
        // A clip with nothing said in it has no subtitles.
        assert!(prepare(vec![cue(0, 1000, "early")], span).is_empty());
    }
}

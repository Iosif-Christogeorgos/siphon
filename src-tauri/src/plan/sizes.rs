//! Sizes measured on the video itself, where [`Recipe::estimate`] would
//! otherwise go by what the site states, scaled to the part, or by a rule of
//! thumb (see `crate::sizing`, which measures them, and
//! docs/decisions.md#size-estimates).
//!
//! Pure data: what was measured is kept with the video it was measured on
//! (`VideoInfo::sizes`) and laid onto every recipe planned from it
//! ([`SizeFacts::apply`]), so the summary, the quality row, a playlist's
//! total and a trimmed part's progress all go by it.

use std::collections::HashMap;

use super::{AudioStep, Recipe, Trim, VideoAction};
use crate::ytdlp::metadata::{Format, VideoInfo};

/// How far either way an encode measured on the same video by the same
/// encoder, but at another size, frame rate or part, can land: the
/// picture's share of the size moves with what is shown, which these change.
pub const RELATED: (f64, f64) = (0.85, 1.2);

/// A stretch of a video, to the millisecond: a trimmed part, or all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Span {
    Whole,
    Part { start_ms: u64, end_ms: u64 },
}

impl Span {
    pub fn of(trim: Option<Trim>) -> Self {
        match trim {
            Some(trim) => Span::Part {
                start_ms: (trim.start * 1000.0).round() as u64,
                end_ms: (trim.end * 1000.0).round() as u64,
            },
            None => Span::Whole,
        }
    }

    /// Start and end in seconds; the whole runs to `duration`.
    pub fn seconds(self, duration: f64) -> (f64, f64) {
        match self {
            Span::Whole => (0.0, duration),
            Span::Part { start_ms, end_ms } => (start_ms as f64 / 1000.0, end_ms as f64 / 1000.0),
        }
    }
}

/// What measuring found for one recipe (see [`SizeFacts::apply`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Measured {
    /// Bytes of the video stream in the part downloaded (with its sound, for
    /// a stream that carries both).
    pub video: Option<u64>,
    /// Bytes of the separate audio stream in the part downloaded.
    pub audio: Option<u64>,
    /// A smart cut: the bytes its re-encoded ends add to the part (or take
    /// away), against the frames they replace.
    pub cut_ends: Option<i64>,
    /// A constant-quality encode: what it comes to, as a multiple of the
    /// rule of thumb's likely size (`Recipe::rule_of_thumb_video`).
    pub encode: Option<f64>,
    /// … measured on this very encode (the same stream, part and settings),
    /// rather than on another of the same video by the same encoder.
    pub encode_exact: bool,
}

/// Everything measured on one video.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SizeFacts {
    /// Bytes of a stream, by its format id, in a stretch of the video.
    parts: HashMap<(String, Span), u64>,
    /// What a smart cut's re-encoded ends add, by stream and stretch.
    cut_ends: HashMap<(String, Span), i64>,
    /// A constant-quality encode's measured size against the rule of thumb's,
    /// by everything that sets it ([`exact_key`]).
    encodes: HashMap<String, f64>,
    /// The same, by encoder only ([`encoder_key`]): the latest measured.
    encoders: HashMap<String, f64>,
}

/// What is worth measuring for a recipe that hasn't been.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wanted {
    /// Streams (format ids) whose bytes in `span` aren't known.
    pub parts: Vec<String>,
    pub span: Option<Span>,
    /// A smart cut whose re-encoded ends aren't known.
    pub cut_ends: bool,
    /// A constant-quality encode whose size isn't known.
    pub encode: bool,
}

impl Wanted {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty() && !self.cut_ends && !self.encode
    }
}

impl SizeFacts {
    pub fn add_part(&mut self, format_id: &str, span: Span, bytes: u64) {
        self.parts.insert((format_id.to_string(), span), bytes);
    }

    pub fn part(&self, format_id: &str, span: Span) -> Option<u64> {
        self.parts.get(&(format_id.to_string(), span)).copied()
    }

    pub fn add_cut_ends(&mut self, format_id: &str, span: Span, bytes: i64) {
        self.cut_ends.insert((format_id.to_string(), span), bytes);
    }

    fn cut_end(&self, format_id: &str, span: Span) -> Option<i64> {
        self.cut_ends.get(&(format_id.to_string(), span)).copied()
    }

    /// `recipe`'s encode came to `factor` times the rule of thumb's likely size.
    pub fn add_encode(&mut self, recipe: &Recipe, factor: f64) {
        if let (Some(exact), Some(encoder)) = (exact_key(recipe), encoder_key(recipe)) {
            self.encodes.insert(exact, factor);
            self.encoders.insert(encoder, factor);
        }
    }

    /// Everything in `other` as well (the newer wins).
    pub fn merge(&mut self, other: &SizeFacts) {
        self.parts
            .extend(other.parts.iter().map(|(key, bytes)| (key.clone(), *bytes)));
        self.cut_ends
            .extend(other.cut_ends.iter().map(|(key, bytes)| (key.clone(), *bytes)));
        self.encodes
            .extend(other.encodes.iter().map(|(key, f)| (key.clone(), *f)));
        self.encoders
            .extend(other.encoders.iter().map(|(key, f)| (key.clone(), *f)));
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.cut_ends.is_empty() && self.encodes.is_empty()
    }

    /// Lays what is known onto `recipe` (see [`Measured`]).
    pub fn apply(&self, recipe: &mut Recipe) {
        let span = Span::of(recipe.trim);
        let mut measured = Measured {
            video: recipe
                .video
                .as_ref()
                .and_then(|video| self.part(&video.format_id, span)),
            audio: match &recipe.audio {
                Some(AudioStep {
                    format_id: Some(id), ..
                }) => self.part(id, span),
                _ => None,
            },
            cut_ends: recipe
                .video
                .as_ref()
                .filter(|_| recipe.smart_cut())
                .and_then(|video| self.cut_end(&video.format_id, span)),
            ..Measured::default()
        };
        if quality_encode(recipe) {
            if let Some(factor) = exact_key(recipe).and_then(|key| self.encodes.get(&key)) {
                measured.encode = Some(*factor);
                measured.encode_exact = true;
            } else if let Some(factor) = encoder_key(recipe).and_then(|key| self.encoders.get(&key)) {
                measured.encode = Some(*factor);
            }
        }
        recipe.measured = measured;
    }

    /// What measuring could still tell about `recipe`'s size, planned from
    /// `video`: the bytes of a trimmed part (or of a stream whose size was
    /// only worked out from its bitrate), and a constant-quality encode.
    /// Only streams Siphon can read itself, and never a stream as it airs.
    pub fn wanted(&self, recipe: &Recipe, video: &VideoInfo) -> Wanted {
        if video.live.is_some() || video.duration.is_none_or(|duration| duration <= 0.0) {
            return Wanted::default();
        }
        let span = Span::of(recipe.trim);
        let format = |id: &str| video.formats.iter().find(|format| format.id == id);
        let readable = |format: &&Format| format.stream_url.is_some();
        // A part's bytes; or the whole of a segmented stream whose size is
        // only its bitrate times its length (Twitch's), read off its segments.
        let worth = |format: &Format| match span {
            Span::Part { .. } => true,
            Span::Whole => format.segmented && format.filesize_estimated,
        };
        let ids = [
            recipe.video.as_ref().map(|video| video.format_id.as_str()),
            recipe.audio.as_ref().and_then(|audio| audio.format_id.as_deref()),
        ];
        let parts: Vec<String> = ids
            .into_iter()
            .flatten()
            .filter_map(format)
            .filter(readable)
            .filter(|format| worth(format))
            .filter(|format| self.part(&format.id, span).is_none())
            .map(|format| format.id.clone())
            .collect();
        let video_readable = recipe
            .video
            .as_ref()
            .and_then(|video| format(&video.format_id))
            .is_some_and(|format| format.stream_url.is_some());
        let cut_ends = recipe.smart_cut()
            && video_readable
            && recipe
                .video
                .as_ref()
                .is_some_and(|video| self.cut_end(&video.format_id, span).is_none());
        let encode = quality_encode(recipe)
            && video_readable
            && exact_key(recipe).is_none_or(|key| !self.encodes.contains_key(&key));
        Wanted {
            span: (!parts.is_empty() || cut_ends).then_some(span),
            parts,
            cut_ends,
            encode,
        }
    }
}

/// True when the recipe's video is encoded at a constant quality, whose size
/// depends on the picture (a size-targeted encode's doesn't).
fn quality_encode(recipe: &Recipe) -> bool {
    matches!(
        recipe.video.as_ref().map(|video| &video.action),
        Some(VideoAction::Encode(encode)) if encode.bitrate_kbps.is_none()
    )
}

/// Everything that sets a constant-quality encode's size: the stream, the
/// part, the encoder and its settings, and the chat drawn in.
fn exact_key(recipe: &Recipe) -> Option<String> {
    let video = recipe.video.as_ref()?;
    let VideoAction::Encode(encode) = &video.action else {
        return None;
    };
    Some(format!(
        "{}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{}",
        video.format_id,
        Span::of(recipe.trim),
        encode.codec,
        encode.hardware,
        encode.fast,
        encode.max_kbps,
        encode.scale_lines,
        encode.max_fps,
        recipe.chat.is_some(),
    ))
}

/// The encoder alone, and whether the chat is drawn in: what an encode of
/// the same video at another size or of another part has in common.
fn encoder_key(recipe: &Recipe) -> Option<String> {
    let encode = recipe.video_encode()?;
    Some(format!(
        "{:?}|{:?}|{}|{}",
        encode.codec,
        encode.hardware,
        encode.fast,
        recipe.chat.is_some()
    ))
}

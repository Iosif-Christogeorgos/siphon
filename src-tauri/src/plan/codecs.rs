//! Codecs and containers as Siphon understands them, and which fit together.

use serde::{Deserialize, Serialize};

use crate::ytdlp::metadata::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    Hevc,
    Vp9,
    Av1,
    Vp8,
    /// Intra-frame editing codec. Only ever an encode target.
    Dnxhr,
    Other,
}

impl VideoCodec {
    /// Reads yt-dlp's codec string ("avc1.640028", "vp09.00.40.08", "av01.0.08M.08", …).
    /// When the site doesn't say, the file extension is a decent guess.
    pub fn of(format: &Format) -> Self {
        let Some(raw) = format.vcodec.as_deref().map(str::to_ascii_lowercase) else {
            return match format.ext.as_str() {
                "mp4" | "m4v" | "mov" => VideoCodec::H264,
                "webm" => VideoCodec::Vp9,
                _ => VideoCodec::Other,
            };
        };
        match raw.as_str() {
            c if c.starts_with("avc") || c.starts_with("h264") => VideoCodec::H264,
            c if c.starts_with("hev") || c.starts_with("hvc") || c.starts_with("h265") || c.starts_with("hevc") => {
                VideoCodec::Hevc
            }
            c if c.starts_with("vp09") || c.starts_with("vp9") => VideoCodec::Vp9,
            c if c.starts_with("av01") || c.starts_with("av1") => VideoCodec::Av1,
            c if c.starts_with("vp08") || c.starts_with("vp8") => VideoCodec::Vp8,
            _ => VideoCodec::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp8 => "VP8",
            VideoCodec::Dnxhr => "DNxHR",
            VideoCodec::Other => "video",
        }
    }

    /// Rough compression efficiency, for ranking streams of equal resolution
    /// when nothing else matters (a newer codec looks better per bit).
    pub fn efficiency(self) -> u8 {
        match self {
            VideoCodec::Av1 => 4,
            VideoCodec::Vp9 | VideoCodec::Hevc => 3,
            VideoCodec::H264 => 2,
            VideoCodec::Vp8 => 1,
            VideoCodec::Dnxhr | VideoCodec::Other => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    Vorbis,
    Flac,
    Ac3,
    /// Uncompressed. Only ever an encode target (WAV, editing files).
    Pcm,
    Other,
}

impl AudioCodec {
    pub fn of(format: &Format) -> Self {
        let Some(raw) = format.acodec.as_deref().map(str::to_ascii_lowercase) else {
            return match format.ext.as_str() {
                "mp4" | "m4a" | "m4v" | "mov" => AudioCodec::Aac,
                "webm" | "opus" => AudioCodec::Opus,
                "mp3" => AudioCodec::Mp3,
                "ogg" => AudioCodec::Vorbis,
                "flac" => AudioCodec::Flac,
                _ => AudioCodec::Other,
            };
        };
        match raw.as_str() {
            c if c.starts_with("mp4a") || c.starts_with("aac") => AudioCodec::Aac,
            c if c.starts_with("opus") => AudioCodec::Opus,
            c if c.starts_with("mp3") => AudioCodec::Mp3,
            c if c.starts_with("vorbis") => AudioCodec::Vorbis,
            c if c.starts_with("flac") => AudioCodec::Flac,
            c if c.starts_with("ac-3") || c.starts_with("ec-3") || c.starts_with("ac3") || c.starts_with("eac3") => {
                AudioCodec::Ac3
            }
            _ => AudioCodec::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Aac => "AAC",
            AudioCodec::Opus => "Opus",
            AudioCodec::Mp3 => "MP3",
            AudioCodec::Vorbis => "Vorbis",
            AudioCodec::Flac => "FLAC",
            AudioCodec::Ac3 => "Dolby Digital",
            AudioCodec::Pcm => "uncompressed PCM",
            AudioCodec::Other => "audio",
        }
    }

    /// Lossless codecs can't add detail that a lossy source already threw away.
    pub fn is_lossless(self) -> bool {
        matches!(self, AudioCodec::Flac | AudioCodec::Pcm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mkv,
    Webm,
    Mov,
    // Audio-only files.
    M4a,
    Mp3,
    Opus,
    Ogg,
    Flac,
    Wav,
}

impl Container {
    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Webm => "webm",
            Container::Mov => "mov",
            Container::M4a => "m4a",
            Container::Mp3 => "mp3",
            Container::Opus => "opus",
            Container::Ogg => "ogg",
            Container::Flac => "flac",
            Container::Wav => "wav",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
            Container::Webm => "WebM",
            Container::Mov => "MOV",
            Container::M4a => "M4A",
            Container::Mp3 => "MP3",
            Container::Opus => "Opus",
            Container::Ogg => "Ogg",
            Container::Flac => "FLAC",
            Container::Wav => "WAV",
        }
    }

    pub fn is_audio_only(self) -> bool {
        !matches!(self, Container::Mp4 | Container::Mkv | Container::Webm | Container::Mov)
    }

    pub fn holds_video(self, codec: VideoCodec) -> bool {
        use VideoCodec::*;
        match self {
            Container::Mkv => true,
            Container::Mp4 => matches!(codec, H264 | Hevc | Av1 | Vp9),
            Container::Webm => matches!(codec, Vp9 | Vp8 | Av1),
            Container::Mov => matches!(codec, H264 | Hevc | Dnxhr),
            _ => false,
        }
    }

    pub fn holds_audio(self, codec: AudioCodec) -> bool {
        use AudioCodec::*;
        match self {
            Container::Mkv => true,
            Container::Mp4 => matches!(codec, Aac | Mp3 | Opus | Ac3 | Flac),
            Container::Webm => matches!(codec, Opus | Vorbis),
            Container::Mov => matches!(codec, Aac | Pcm | Ac3),
            Container::M4a => codec == Aac,
            Container::Mp3 => codec == Mp3,
            Container::Opus => codec == Opus,
            Container::Ogg => codec == Vorbis,
            Container::Flac => codec == Flac,
            Container::Wav => codec == Pcm,
        }
    }

    /// What to convert to when the wanted codec doesn't fit this container.
    pub fn default_video(self) -> VideoCodec {
        match self {
            Container::Webm => VideoCodec::Vp9,
            _ => VideoCodec::H264,
        }
    }

    pub fn default_audio(self) -> AudioCodec {
        match self {
            Container::Webm | Container::Opus => AudioCodec::Opus,
            Container::Ogg => AudioCodec::Vorbis,
            Container::Mp3 => AudioCodec::Mp3,
            Container::Flac => AudioCodec::Flac,
            Container::Wav => AudioCodec::Pcm,
            _ => AudioCodec::Aac,
        }
    }

    /// The tidiest container that holds both streams without conversion.
    pub fn natural(video: Option<VideoCodec>, audio: Option<AudioCodec>) -> Container {
        match video {
            None => match audio {
                Some(AudioCodec::Aac) => Container::M4a,
                Some(AudioCodec::Opus) => Container::Opus,
                Some(AudioCodec::Mp3) => Container::Mp3,
                Some(AudioCodec::Vorbis) => Container::Ogg,
                Some(AudioCodec::Flac) => Container::Flac,
                _ => Container::Mkv,
            },
            Some(video) => [Container::Mp4, Container::Webm]
                .into_iter()
                // Opus-in-MP4 is legal but poorly supported; WebM is its home.
                .filter(|c| !(*c == Container::Mp4 && audio == Some(AudioCodec::Opus)))
                .find(|c| c.holds_video(video) && audio.is_none_or(|a| c.holds_audio(a)))
                .unwrap_or(Container::Mkv),
        }
    }
}

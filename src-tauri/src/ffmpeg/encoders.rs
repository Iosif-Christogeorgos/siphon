//! Finds out which hardware video encoders actually work on this machine.
//!
//! ffmpeg lists every encoder it was built with, whether or not the GPU and
//! driver are present, so the only reliable test is to encode a few frames.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::plan::codecs::VideoCodec;
use crate::process;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hardware {
    /// NVIDIA
    Nvenc,
    /// Intel Quick Sync
    Qsv,
    /// AMD
    Amf,
    /// Apple
    VideoToolbox,
    /// libx264 / libx265 on the CPU.
    Software,
}

impl Hardware {
    /// Tried in this order; the first that encodes successfully wins.
    const CANDIDATES: &'static [Hardware] = if cfg!(target_os = "macos") {
        &[Hardware::VideoToolbox]
    } else {
        &[Hardware::Nvenc, Hardware::Qsv, Hardware::Amf]
    };

    /// ffmpeg's encoder name for `codec`, e.g. "hevc_nvenc". Codecs without
    /// hardware support here always use their software encoder.
    pub fn encoder(self, codec: VideoCodec) -> &'static str {
        match (codec, self) {
            (VideoCodec::H264, Hardware::Nvenc) => "h264_nvenc",
            (VideoCodec::H264, Hardware::Qsv) => "h264_qsv",
            (VideoCodec::H264, Hardware::Amf) => "h264_amf",
            (VideoCodec::H264, Hardware::VideoToolbox) => "h264_videotoolbox",
            (VideoCodec::Hevc, Hardware::Nvenc) => "hevc_nvenc",
            (VideoCodec::Hevc, Hardware::Qsv) => "hevc_qsv",
            (VideoCodec::Hevc, Hardware::Amf) => "hevc_amf",
            (VideoCodec::Hevc, Hardware::VideoToolbox) => "hevc_videotoolbox",
            (VideoCodec::Hevc, Hardware::Software) => "libx265",
            (VideoCodec::Vp9 | VideoCodec::Vp8, _) => "libvpx-vp9",
            (VideoCodec::Av1, _) => "libsvtav1",
            (VideoCodec::Dnxhr, _) => "dnxhd",
            _ => "libx264",
        }
    }
}

/// The encoder to use for each codec that has hardware options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoders {
    pub h264: Hardware,
    pub hevc: Hardware,
}

impl Encoders {
    pub const SOFTWARE: Encoders = Encoders {
        h264: Hardware::Software,
        hevc: Hardware::Software,
    };

    pub fn for_codec(&self, codec: VideoCodec) -> Hardware {
        match codec {
            VideoCodec::H264 => self.h264,
            VideoCodec::Hevc => self.hevc,
            _ => Hardware::Software,
        }
    }
}

/// Probes the hardware encoders. Takes a second or two; callers cache the result.
pub async fn detect(ffmpeg: &Path) -> Encoders {
    let (h264, hevc) = tokio::join!(
        first_working(ffmpeg, VideoCodec::H264),
        first_working(ffmpeg, VideoCodec::Hevc)
    );
    Encoders { h264, hevc }
}

async fn first_working(ffmpeg: &Path, codec: VideoCodec) -> Hardware {
    for &hardware in Hardware::CANDIDATES {
        if works(ffmpeg, hardware.encoder(codec)).await {
            return hardware;
        }
    }
    Hardware::Software
}

/// Encodes three synthetic frames and reports whether ffmpeg succeeded.
async fn works(ffmpeg: &Path, encoder: &str) -> bool {
    let mut cmd = process::command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=black:size=640x360:rate=30",
    ])
    .args(["-frames:v", "3", "-c:v", encoder, "-f", "null", "-"])
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    matches!(
        tokio::time::timeout(Duration::from_secs(20), cmd.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

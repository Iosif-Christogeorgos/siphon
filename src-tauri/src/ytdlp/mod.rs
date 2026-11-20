//! Everything that talks to the yt-dlp executable.
//!
//! Arguments are always passed as an array, one value per element. Nothing
//! here (or anywhere else) builds a shell string from user input.

pub mod args;
pub mod errors;
pub mod metadata;
pub mod progress;
pub mod subtitles;

use tokio::process::Command;

use crate::binaries::BinaryManager;
use crate::error::{AppError, ErrorKind, Result};
use crate::process;

/// A yt-dlp command with the flags every invocation needs.
pub fn command(bins: &BinaryManager) -> Command {
    let mut cmd = process::command(bins.yt_dlp());
    cmd.args([
        // Behave the same on every machine, whatever config the user has.
        "--ignore-config",
        // We parse the output; keep it free of colour codes and in UTF-8
        // (Windows would otherwise use the legacy console code page).
        "--color",
        "never",
        "--encoding",
        "utf-8",
    ]);
    // YouTube needs a JavaScript runtime; point yt-dlp at our managed Deno.
    cmd.arg("--js-runtimes").arg(format!("deno:{}", bins.deno().display()));
    cmd.arg("--ffmpeg-location").arg(bins.ffmpeg_dir());
    cmd
}

/// Accepts only plain http(s) links. Combined with the `--` separator the
/// callers put before the URL, user input can never be read as an option.
pub fn validate_url(url: &str) -> Result<&str> {
    let url = url.trim();
    let is_http = url.starts_with("https://") || url.starts_with("http://");
    if !is_http || url.chars().any(char::is_whitespace) || url.len() > 2048 {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "That doesn't look like a link Siphon can open.",
        ));
    }
    Ok(url)
}

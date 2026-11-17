//! Where each managed tool comes from: official GitHub releases only.

use serde::{Deserialize, Serialize};

use super::Tool;
use crate::error::{AppError, Result};

/// Release channel. Only yt-dlp publishes nightlies; ffmpeg and Deno ignore it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Nightly,
}

/// A downloadable build of a tool, resolved from the GitHub API.
#[derive(Debug, Clone)]
pub struct Release {
    /// Identity used to decide "is this newer than what's installed".
    pub version: String,
    /// Release notes (markdown), when the project publishes any.
    pub notes: Option<String>,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
    /// URL of the project's published SHA-256 list for this release.
    pub checksums_url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    updated_at: String,
}

struct Source {
    repo: &'static str,
    asset: &'static str,
    checksums: String,
    /// ffmpeg ships from a rolling `latest` tag, so its tag says nothing;
    /// the asset's upload date identifies the build instead.
    rolling: bool,
}

fn source(tool: Tool, channel: Channel) -> Option<Source> {
    use std::env::consts::{ARCH, OS};
    let (repo, asset, checksums, rolling) = match (tool, OS, ARCH) {
        (Tool::YtDlp, _, _) => {
            let repo = match channel {
                Channel::Stable => "yt-dlp/yt-dlp",
                Channel::Nightly => "yt-dlp/yt-dlp-nightly-builds",
            };
            let asset = match (OS, ARCH) {
                ("windows", "x86_64") => "yt-dlp.exe",
                ("windows", "aarch64") => "yt-dlp_arm64.exe",
                _ => return None,
            };
            (repo, asset, "SHA2-256SUMS".to_string(), false)
        }
        // The `-shared` build keeps the codecs in DLLs shared by ffmpeg and
        // ffprobe: under half the download of the static build.
        (Tool::Ffmpeg, "windows", "x86_64") => {
            let asset = "ffmpeg-master-latest-win64-gpl-shared.zip";
            ("yt-dlp/FFmpeg-Builds", asset, "checksums.sha256".to_string(), true)
        }
        (Tool::Ffmpeg, "windows", "aarch64") => {
            let asset = "ffmpeg-master-latest-winarm64-gpl-shared.zip";
            ("yt-dlp/FFmpeg-Builds", asset, "checksums.sha256".to_string(), true)
        }
        (Tool::Deno, os, arch) => {
            let asset = match (os, arch) {
                ("windows", "x86_64") => "deno-x86_64-pc-windows-msvc.zip",
                ("windows", "aarch64") => "deno-aarch64-pc-windows-msvc.zip",
                _ => return None,
            };
            ("denoland/deno", asset, format!("{asset}.sha256sum"), false)
        }
        _ => return None,
    };
    Some(Source {
        repo,
        asset,
        checksums,
        rolling,
    })
}

/// Looks up the newest release of `tool` on GitHub.
pub async fn latest(http: &reqwest::Client, tool: Tool, channel: Channel) -> Result<Release> {
    let source = source(tool, channel)
        .ok_or_else(|| AppError::setup(format!("{} isn't available for this platform.", tool.label())))?;

    let url = format!("https://api.github.com/repos/{}/releases/latest", source.repo);
    let response = http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::new(
            crate::error::ErrorKind::Network,
            format!("Couldn't check for {} updates.", tool.label()),
        )
        .with_details(format!("GET {url} → {status}\n{body}")));
    }
    let release: GithubRelease = response.json().await?;

    let find = |name: &str| release.assets.iter().find(|asset| asset.name == name);
    let missing = |name: &str| AppError::setup(format!("The latest {} release is missing “{name}”.", tool.label()));
    let asset = find(source.asset).ok_or_else(|| missing(source.asset))?;
    let checksums = find(&source.checksums).ok_or_else(|| missing(&source.checksums))?;

    let version = if source.rolling {
        // "2026-11-15T21:56:48Z" → "2026-11-15"
        asset
            .updated_at
            .split('T')
            .next()
            .unwrap_or(&asset.updated_at)
            .to_string()
    } else {
        release.tag_name.clone()
    };

    Ok(Release {
        version,
        notes: release.body.clone().filter(|body| !body.trim().is_empty()),
        asset_name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        size: asset.size,
        checksums_url: checksums.browser_download_url.clone(),
    })
}

/// Finds the SHA-256 of `asset_name` in a published checksum file.
///
/// Three formats are in the wild:
/// - `<hash>  <file>` lines (yt-dlp's `SHA2-256SUMS`, FFmpeg-Builds, Deno on Unix)
/// - PowerShell `Get-FileHash` output, `Hash : <HASH>` (Deno on Windows)
pub fn find_sha256(checksums: &str, asset_name: &str) -> Option<String> {
    let is_hash = |token: &str| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit());

    for line in checksums.lines() {
        let mut tokens = line.split_whitespace();
        let (Some(hash), Some(name)) = (tokens.next(), tokens.next_back()) else {
            continue;
        };
        // The name must match exactly: "yt-dlp" is a prefix of "yt-dlp.exe".
        if is_hash(hash) && name.trim_start_matches('*') == asset_name {
            return Some(hash.to_ascii_lowercase());
        }
    }

    // Single-file checksum (Get-FileHash style): accept it only if unambiguous.
    let mut hashes = checksums.split_whitespace().filter(|token| is_hash(token));
    match (hashes.next(), hashes.next()) {
        (Some(hash), None) if checksums.contains("SHA256") => Some(hash.to_ascii_lowercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::find_sha256;

    const A: &str = "e51e69e89489f199530f13d32a4342da3ddb2cf34842f364603a066db05afbf3";
    const B: &str = "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d";

    #[test]
    fn matches_exact_file_name_in_sums_list() {
        let sums = format!("{A}  yt-dlp\n{B}  yt-dlp.exe\n");
        assert_eq!(find_sha256(&sums, "yt-dlp.exe").as_deref(), Some(B));
        assert_eq!(find_sha256(&sums, "yt-dlp").as_deref(), Some(A));
        assert_eq!(find_sha256(&sums, "yt-dlp_x86.exe"), None);
    }

    #[test]
    fn reads_powershell_get_filehash_output() {
        let sums = format!(
            "\nAlgorithm : SHA256\nHash      : {}\nPath      : C:\\a\\deno\\deno.zip\n\n",
            A.to_ascii_uppercase()
        );
        assert_eq!(
            find_sha256(&sums, "deno-x86_64-pc-windows-msvc.zip").as_deref(),
            Some(A)
        );
    }
}

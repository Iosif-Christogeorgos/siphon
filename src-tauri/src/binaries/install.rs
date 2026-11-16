//! Download → verify → unpack → atomic swap → self-test → (rollback).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::sources::{find_sha256, Release};
use super::{exe, BinaryManager, Tool, ToolState};
use crate::error::{AppError, ErrorKind, Result};
use crate::process;

/// The first launch of a freshly downloaded executable can be slow while
/// antivirus scans it (and yt-dlp unpacks itself), so be generous.
const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(90);

pub async fn install(manager: &BinaryManager, tool: Tool, release: &Release) -> Result<()> {
    let current = manager.tool_dir(tool);
    let staging = sibling(&current, "staging");
    let previous = sibling(&current, "previous");

    let result = stage(manager, tool, release, &staging).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    result?;

    // Swapping needs the tool to itself. If a download is using it, give up
    // for now (the caller tries again later) instead of making it wait.
    let Ok(_exclusive) = manager.use_gate.try_write() else {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(AppError::new(
            ErrorKind::Busy,
            format!(
                "{} will be updated once the current downloads have finished.",
                tool.label()
            ),
        ));
    };
    swap_in(&current, &staging, &previous).await?;

    if let Err(err) = self_test(tool, &current).await {
        roll_back(&current, &previous).await;
        return Err(err);
    }

    // The previous yt-dlp/Deno stay around as a known-good fallback. ffmpeg is
    // a few hundred MB and its builds don't break in practice, so drop its copy.
    if tool == Tool::Ffmpeg {
        let _ = tokio::fs::remove_dir_all(&previous).await;
    }
    Ok(())
}

/// `bin/yt-dlp` → `bin/yt-dlp.staging`
fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    dir.with_file_name(name)
}

/// Fills `staging` with a verified, ready-to-run copy of the release.
async fn stage(manager: &BinaryManager, tool: Tool, release: &Release, staging: &Path) -> Result<()> {
    let _ = tokio::fs::remove_dir_all(staging).await;
    tokio::fs::create_dir_all(staging).await?;
    let archive = staging.join(&release.asset_name);

    manager.set_state(tool, ToolState::Downloading);
    let actual = download(manager, tool, release, &archive).await?;

    manager.set_state(tool, ToolState::Verifying);
    let checksums = manager
        .http
        .get(&release.checksums_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let expected = find_sha256(&checksums, &release.asset_name).ok_or_else(|| {
        AppError::setup(format!(
            "No published checksum for {}, so it wasn't installed.",
            tool.label()
        ))
        .with_details(checksums.clone())
    })?;
    if actual != expected {
        return Err(AppError::setup(format!(
            "The {} download didn't match its published checksum, so it wasn't installed.",
            tool.label()
        ))
        .with_details(format!("expected {expected}\nactual   {actual}")));
    }

    manager.set_state(tool, ToolState::Installing);
    if release.asset_name.ends_with(".zip") {
        let (zip_path, dest) = (archive.clone(), staging.to_path_buf());
        tokio::task::spawn_blocking(move || unzip(tool, &zip_path, &dest))
            .await
            .map_err(|err| AppError::internal("Unpacking was interrupted.").with_details(err.to_string()))??;
        tokio::fs::remove_file(&archive).await?;
    } else {
        // A bare executable (yt-dlp): just give it its canonical name.
        tokio::fs::rename(&archive, staging.join(exe(tool.executables()[0]))).await?;
    }

    for name in tool.executables() {
        let path = staging.join(exe(name));
        if !path.is_file() {
            return Err(AppError::setup(format!(
                "The {} download didn't contain {name}.",
                tool.label()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).await?;
        }
    }
    Ok(())
}

/// Streams the asset to disk, hashing as it goes. Returns the hex SHA-256.
async fn download(manager: &BinaryManager, tool: Tool, release: &Release, dest: &Path) -> Result<String> {
    let response = manager
        .http
        .get(&release.download_url)
        .send()
        .await?
        .error_for_status()?;
    let total = response.content_length().unwrap_or(release.size).max(1);

    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut last_emit = Instant::now();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;

        if last_emit.elapsed() >= Duration::from_millis(80) {
            last_emit = Instant::now();
            let fraction = (received as f64 / total as f64).min(1.0);
            manager.update_status(tool, |status| status.progress = Some(fraction));
        }
    }
    file.flush().await?;
    Ok(hex::encode(hasher.finalize()))
}

/// Extracts just the files a tool needs, flattened into `dest`.
fn unzip(tool: Tool, archive: &Path, dest: &Path) -> Result<()> {
    let unzip_error =
        |err: zip::result::ZipError| AppError::setup("Couldn't unpack the download.").with_details(err.to_string());

    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?).map_err(unzip_error)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(unzip_error)?;
        if !entry.is_file() {
            continue;
        }
        // `enclosed_name` rejects paths that would escape the destination.
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()).map(str::to_owned) else {
            continue;
        };

        let wanted = match tool {
            // FFmpeg-Builds layout: <root>/bin/{ffmpeg,ffprobe,ffplay}.exe + DLLs,
            // next to doc/, include/ and lib/, which we don't need.
            Tool::Ffmpeg => {
                let in_bin = path.parent().and_then(Path::file_name).is_some_and(|dir| dir == "bin");
                in_bin && !file_name.starts_with("ffplay")
            }
            _ => tool.executables().iter().any(|name| exe(name) == file_name),
        };
        if wanted {
            let mut out = std::fs::File::create(dest.join(&file_name))?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

/// Moves `current` aside and `staging` into place. Each step is a rename on
/// the same volume, so there is never a half-written tool folder.
async fn swap_in(current: &Path, staging: &Path, previous: &Path) -> Result<()> {
    let had_current = current.exists();
    if had_current {
        let _ = tokio::fs::remove_dir_all(previous).await;
        tokio::fs::rename(current, previous).await.map_err(|err| {
            AppError::setup("Couldn't replace the old version (is it in use?).").with_details(err.to_string())
        })?;
    }
    if let Err(err) = tokio::fs::rename(staging, current).await {
        if had_current {
            let _ = tokio::fs::rename(previous, current).await;
        }
        return Err(err.into());
    }
    Ok(())
}

/// Puts the last known-good version back after a failed self-test.
async fn roll_back(current: &Path, previous: &Path) {
    let _ = tokio::fs::remove_dir_all(current).await;
    if previous.exists() {
        let _ = tokio::fs::rename(previous, current).await;
    }
}

/// Runs every executable of the tool once to prove it starts.
async fn self_test(tool: Tool, dir: &Path) -> Result<()> {
    for name in tool.executables() {
        let failed = |details: String| {
            AppError::setup(format!(
                "The new {} didn't start, so the previous version was kept.",
                tool.label()
            ))
            .with_details(details)
        };
        let mut cmd = process::command(dir.join(exe(name)));
        cmd.arg(tool.version_flag());
        match tokio::time::timeout(SELF_TEST_TIMEOUT, cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                return Err(failed(format!(
                    "{name} exited with {}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )))
            }
            Ok(Err(err)) => return Err(failed(err.to_string())),
            Err(_) => return Err(failed(format!("{name} timed out"))),
        }
    }
    Ok(())
}

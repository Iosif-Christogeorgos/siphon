//! Child-process helpers shared by everything that runs a managed binary.

use std::ffi::OsStr;
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::process::Command;

/// `CREATE_NO_WINDOW`: without it every yt-dlp/ffmpeg launch flashes a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command with no stdin and no console window.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Runs a command to completion and captures its output. Returns `None` if
/// it outlives `limit`, in which case its whole process tree is killed.
pub async fn output_within(mut cmd: Command, limit: Duration) -> std::io::Result<Option<Output>> {
    let child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let pid = child.id();
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(output) => output.map(Some),
        Err(_) => {
            if let Some(pid) = pid {
                kill_tree(pid).await;
            }
            Ok(None)
        }
    }
}

/// Kills a process and all of its descendants.
///
/// yt-dlp's Windows build is a self-extracting launcher that spawns the real
/// interpreter, which in turn spawns ffmpeg. Killing only the launcher would
/// leave the others running, so the whole tree has to go.
pub async fn kill_tree(pid: u32) {
    let mut killer = command("taskkill");
    killer.args(["/PID", &pid.to_string(), "/T", "/F"]);
    let _ = killer.stdout(Stdio::null()).stderr(Stdio::null()).status().await;
}

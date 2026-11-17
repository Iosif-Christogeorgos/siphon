//! A short record of decisions worth knowing later, such as which way a cut
//! was made: `%LOCALAPPDATA%\app.siphon.desktop\logs\siphon.log`. It stays on
//! this computer. When it passes 1 MB it is renamed `siphon.old.log` and a
//! new one is started.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

const MAX_SIZE: u64 = 1024 * 1024;

/// Adds one line, prefixed with the time (UTC). Failing to write is not an error.
pub fn line(app: &AppHandle, message: &str) {
    let Ok(dir) = app.path().app_log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("siphon.log");
    if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > MAX_SIZE) {
        let _ = std::fs::rename(&file, dir.join("siphon.old.log"));
    }
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(out, "{} {message}", timestamp(seconds));
    }
}

/// `2026-11-17 20:31:05` for a count of seconds since 1970 (UTC).
fn timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_readable_times() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(timestamp(1_794_947_465), "2026-11-17 20:31:05");
    }
}

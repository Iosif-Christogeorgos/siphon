//! One error type for everything that crosses the IPC boundary.
//!
//! The frontend shows `message` (plain language) and tucks `details`
//! (the raw log) behind a "details" toggle.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The site protects this video with DRM. Siphon never tries to bypass it.
    Drm,
    /// yt-dlp has no extractor for this link.
    Unsupported,
    /// Removed, private, geo-blocked or otherwise not reachable.
    Unavailable,
    /// The site wants an account (age gate, members-only, private).
    LoginRequired,
    /// Errors that usually go away after updating yt-dlp (bot check, missing formats, throttling).
    Outdated,
    Network,
    DiskFull,
    Cancelled,
    /// A managed binary is missing or broken.
    Setup,
    Internal,
}

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    pub details: Option<String>,
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn setup(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Setup, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "Cancelled")
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        let details = details.into();
        if !details.trim().is_empty() {
            self.details = Some(details);
        }
        self
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::StorageFull => ErrorKind::DiskFull,
            _ => ErrorKind::Internal,
        };
        let message = match kind {
            ErrorKind::DiskFull => "The disk is full.",
            _ => "A file operation failed.",
        };
        Self::new(kind, message).with_details(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        Self::new(
            ErrorKind::Network,
            "Couldn't reach the server. Check your connection.",
        )
        .with_details(format!("{err:?}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::internal("Received data in an unexpected format.").with_details(err.to_string())
    }
}

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
    /// The site's own account can unlock it (age gate, members-only, logged-in viewers).
    /// Not private or paid videos: signing in doesn't help with those.
    LoginRequired,
    /// Errors that usually go away after updating yt-dlp (bot check, missing formats, throttling).
    Outdated,
    Network,
    DiskFull,
    Cancelled,
    /// A managed binary is missing or broken.
    Setup,
    /// Can't be done right now because something else is using it.
    Busy,
    /// Share: too long to fit the size limit at a quality worth having.
    TooLong,
    Internal,
}

/// Whether doing the same thing again may work, and how soon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Retry {
    #[default]
    No,
    /// A dropped or refused connection: worth another go within seconds.
    Soon,
    /// The site is holding requests back: give it a while first.
    Later,
}

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    pub details: Option<String>,
    /// Set when signing in to a site could fix this: the frontend offers it.
    pub sign_in: Option<SignInNeed>,
    /// For the backend's own use; the frontend only sees errors that are final.
    #[serde(skip)]
    pub retry: Retry,
    /// Whether an account could help, as the error's wording tells (see
    /// `ytdlp::errors`). `sign_in` is made from it once the site is known.
    #[serde(skip)]
    pub chance: Chance,
}

/// How likely signing in to the site is to fix an error. Siphon always tries
/// without an account first, and only offers one after the site refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Chance {
    /// An account can't help: DRM, removed, region-blocked, a connection problem.
    #[default]
    Never,
    /// A login wall: an age gate, posts for signed-in viewers only.
    Needed,
    /// Only if the account has access: members-only, Premium-only, a private
    /// video shared with it, a supporter tier.
    Maybe,
    /// The bot check. Signing in often gets past it, but it's also what gets
    /// accounts flagged, so it's offered with that warning, never suggested.
    Risky,
    /// An error Siphon doesn't recognise. An account rarely helps; offered quietly.
    Unknown,
}

/// The site to sign in to, for an error an account could fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInNeed {
    /// "youtube.com": what the user signs in to, and where the cookies go.
    pub site: String,
    pub chance: Chance,
    /// Siphon is signed in to the site already.
    pub signed_in: bool,
    /// The site refused even with the sign-in: for `Needed` the session ran
    /// out, for `Maybe` the account has no access.
    pub refused: bool,
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: None,
            sign_in: None,
            chance: Chance::Never,
            // Connections fail for reasons that pass; nothing else does by default.
            retry: if kind == ErrorKind::Network {
                Retry::Soon
            } else {
                Retry::No
            },
        }
    }

    pub fn retrying(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
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
        Self::new(ErrorKind::Network, "Couldn't reach the server. Check your connection.")
            .with_details(format!("{err:?}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::internal("Received data in an unexpected format.").with_details(err.to_string())
    }
}

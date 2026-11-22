//! Twitch chat, drawn into the video.
//!
//! A Twitch video or recording with chat becomes one file with the chat
//! drawn into the picture, in the box the user placed on the preview. This
//! module fetches the chat (a VOD's replay from Twitch's GraphQL API, see
//! [`twitch`]), the emotes and badges it shows (Twitch's own, and 7TV's,
//! BTTV's and FFZ's, see [`emotes`]), and draws it as a layer that ffmpeg
//! lays over the picture (see [`render`]). How, why and what was measured:
//! docs/chat.md.

pub mod emotes;
pub mod images;
pub mod irc;
pub mod preview;
pub mod render;
mod text;
pub mod twitch;

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// Where the chat goes on the picture, and how dark its box is. Positions
/// and sizes are shares of the frame (0–1), so a placement means the same at
/// 720p and at 4K. The text scales with the box's width; its height decides
/// how many lines show.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The box's left edge, as a share of the frame's width.
    pub x: f64,
    /// The box's top edge, as a share of the frame's height.
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// How dark the box behind the chat is: 0 is clear, 1 is black.
    pub shade: f64,
}

impl Placement {
    /// Down the right edge, over what is usually the least busy part of a
    /// stream, mostly clear.
    pub const DEFAULT: Placement = Placement {
        x: 0.765,
        y: 0.3,
        width: 0.22,
        height: 0.64,
        shade: 0.3,
    };

    /// Narrower than this and a line holds a word or two.
    const MIN_WIDTH: f64 = 0.08;
    const MIN_HEIGHT: f64 = 0.08;

    /// The placement kept inside the frame and within sensible sizes,
    /// whatever was saved or sent.
    pub fn sanitized(self) -> Placement {
        let finite = |value: f64, default: f64| if value.is_finite() { value } else { default };
        let width = finite(self.width, Self::DEFAULT.width).clamp(Self::MIN_WIDTH, 1.0);
        let height = finite(self.height, Self::DEFAULT.height).clamp(Self::MIN_HEIGHT, 1.0);
        Placement {
            x: finite(self.x, Self::DEFAULT.x).clamp(0.0, 1.0 - width),
            y: finite(self.y, Self::DEFAULT.y).clamp(0.0, 1.0 - height),
            width,
            height,
            shade: finite(self.shade, Self::DEFAULT.shade).clamp(0.0, 1.0),
        }
    }
}

/// Where a video's chat comes from: only Twitch's is drawn in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatSource {
    /// A Twitch VOD's chat replay: the VOD's number, and the channel's login.
    Vod { id: String, channel: String },
    /// A Twitch channel's chat as it airs.
    Live { channel: String },
}

impl ChatSource {
    /// The chat of what yt-dlp looked up, from its extractor, the video's id
    /// and the uploader's login, if it is a Twitch VOD or stream.
    pub fn of(extractor: Option<&str>, id: Option<&str>, login: Option<&str>) -> Option<ChatSource> {
        let channel = login.filter(|login| !login.is_empty())?.to_ascii_lowercase();
        match extractor? {
            // yt-dlp writes a VOD's id as "v2929613567".
            "TwitchVod" => {
                let id = id?.trim_start_matches('v');
                (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| ChatSource::Vod {
                    id: id.to_string(),
                    channel,
                })
            }
            "TwitchStream" => Some(ChatSource::Live { channel }),
            _ => None,
        }
    }

    /// The chat of an offline channel's link (`twitch.tv/name`), whose
    /// stream Siphon waits for: the link is all there is to go on.
    pub fn of_channel_link(url: &str) -> Option<ChatSource> {
        let rest = url.split_once("://")?.1;
        let (host, path) = rest.split_once('/')?;
        if !matches!(
            host.to_ascii_lowercase().as_str(),
            "twitch.tv" | "www.twitch.tv" | "m.twitch.tv"
        ) {
            return None;
        }
        let path = path.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
        let login = path.to_ascii_lowercase();
        let reserved = [
            "videos",
            "directory",
            "settings",
            "search",
            "downloads",
            "p",
            "jobs",
            "turbo",
        ];
        let valid = (1..=25).contains(&login.len())
            && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !reserved.contains(&login.as_str());
        valid.then_some(ChatSource::Live { channel: login })
    }
}

/// The chat a recipe draws into its video, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatStep {
    pub source: ChatSource,
    pub placement: Placement,
}

/// A video's chat as it was fetched, with the emotes and badges it shows.
/// Kept in the download's work folder until the file is made, so a paused or
/// failed download doesn't fetch it again.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatLog {
    /// In time order.
    pub messages: Vec<Message>,
    /// The channel's and everyone's third-party emotes, as they were when the
    /// chat was fetched, the one that wins first where two share a name
    /// (see [`emotes::merge`]).
    pub emotes: Vec<Emote>,
    pub badges: Vec<Badge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    /// Seconds into the video when it was sent.
    pub time: f64,
    /// The name as the user writes it ("KaiCenat").
    pub name: String,
    pub login: String,
    /// The colour the user picked, as `#RRGGBB`; none for those who never did.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub badges: Vec<BadgeRef>,
    pub parts: Vec<Part>,
    /// Twitch's own line about something that happened (a sub, a raid),
    /// drawn in its own colour, without a name.
    #[serde(default)]
    pub notice: bool,
}

/// A run of a message: text, or one of Twitch's own emotes. Third-party
/// emotes are words in the text, recognised when the chat is drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    Emote { id: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BadgeRef {
    pub set: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub set: String,
    pub version: String,
    pub images: Vec<Image>,
}

/// A third-party emote (7TV's, BTTV's or FFZ's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Emote {
    /// The word that shows it.
    pub name: String,
    /// The image in each size offered, smallest first.
    pub images: Vec<Image>,
    /// Drawn over the emote before it instead of beside it (7TV's
    /// "zero-width" emotes: a hat, rain, a frame).
    #[serde(default)]
    pub overlay: bool,
}

/// One size of an emote or badge image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    /// Its height in pixels, as the service names it.
    pub height: u32,
    pub url: String,
}

/// The image of `images` to draw at `height` pixels: the smallest that
/// needn't be enlarged (much), or the largest there is.
pub fn image_for(images: &[Image], height: f64) -> Option<&Image> {
    images
        .iter()
        .find(|image| f64::from(image.height) >= height * 0.9)
        .or_else(|| images.last())
}

/// The images of one of Twitch's own emotes: its CDN has three sizes, 28,
/// 56 and 112 pixels high. "default" is the animated version, where there is one.
pub fn twitch_emote_images(id: &str) -> Vec<Image> {
    [(28, "1.0"), (56, "2.0"), (112, "3.0")]
        .into_iter()
        .map(|(height, scale)| Image {
            height,
            url: format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/{scale}"),
        })
        .collect()
}

impl ChatLog {
    /// A channel's chat before anything is said: the third-party emotes and
    /// the badges it shows (`channel_id` is Twitch's number for the
    /// channel). Also says which of those lists couldn't be had: the chat is
    /// drawn without them.
    pub async fn of_channel(twitch: &twitch::Twitch, channel_id: &str) -> (ChatLog, Vec<&'static str>) {
        let ((emotes, mut missing), badges) =
            tokio::join!(emotes::fetch(twitch.http(), channel_id), twitch.badges(channel_id));
        let badges = badges.unwrap_or_else(|_| {
            missing.push("badges");
            Vec::new()
        });
        let log = ChatLog {
            messages: Vec::new(),
            emotes,
            badges,
        };
        (log, missing)
    }

    pub async fn save(&self, file: &Path) -> Result<()> {
        let json = serde_json::to_vec(self).map_err(|err| AppError::internal(err.to_string()))?;
        let partial = file.with_extension("json.part");
        tokio::fs::write(&partial, json).await?;
        tokio::fs::rename(&partial, file).await?;
        Ok(())
    }

    pub async fn load(file: &Path) -> Option<ChatLog> {
        let bytes = tokio::fs::read(file).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The third-party emotes by the word that shows them.
    pub fn emotes_by_name(&self) -> HashMap<&str, &Emote> {
        let mut by_name = HashMap::new();
        for emote in &self.emotes {
            by_name.entry(emote.name.as_str()).or_insert(emote);
        }
        by_name
    }

    /// Every image the chat between `from` and `to` seconds shows: the emotes
    /// its messages use and their senders' badges, at `height` pixels. Only
    /// these are downloaded.
    pub fn images_used(&self, from: f64, to: f64, emote_height: f64, badge_height: f64) -> Vec<String> {
        let emotes = self.emotes_by_name();
        let badges: HashMap<(&str, &str), &Badge> = self
            .badges
            .iter()
            .map(|badge| ((badge.set.as_str(), badge.version.as_str()), badge))
            .collect();
        let mut urls: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut add = |url: Option<&Image>| {
            if let Some(image) = url {
                if seen.insert(image.url.clone()) {
                    urls.push(image.url.clone());
                }
            }
        };
        for message in self.messages.iter().filter(|m| m.time >= from && m.time <= to) {
            for badge in &message.badges {
                if let Some(found) = badges.get(&(badge.set.as_str(), badge.version.as_str())) {
                    add(image_for(&found.images, badge_height));
                }
            }
            for part in &message.parts {
                match part {
                    Part::Emote { id, .. } => add(image_for(&twitch_emote_images(id), emote_height)),
                    Part::Text { text } => {
                        for word in text.split_whitespace() {
                            if let Some(emote) = emotes.get(word) {
                                add(image_for(&emote.images, emote_height));
                            }
                        }
                    }
                }
            }
        }
        urls
    }
}

/// How long after a failure that may pass (a dropped connection, a server
/// error, "too many requests") a request to Twitch or an emote service is
/// tried again: twice, a little later each time.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];

/// Sends the request `request` builds, and again while it fails in a way
/// that may pass (see [`RETRY_AFTER`]). The last answer, whatever its
/// status, or the last error.
pub async fn send(request: impl Fn() -> reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let may_pass = |sent: &reqwest::Result<reqwest::Response>| match sent {
        Ok(response) => {
            response.status().is_server_error() || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        }
        Err(_) => true,
    };
    let mut sent = request().send().await;
    for wait in RETRY_AFTER {
        if !may_pass(&sent) {
            break;
        }
        tokio::time::sleep(wait).await;
        sent = request().send().await;
    }
    sent
}

/// Seconds since 1970 for Twitch's timestamps (`2026-11-15T15:09:44.979Z`),
/// which are always UTC.
pub fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.splitn(3, ':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: f64 = time.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || !(0.0..61.0).contains(&second)
    {
        return None;
    }
    // Days from a civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hour * 3600 + minute * 60) as f64 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_twitch_timestamps() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_timestamp("2026-11-15T14:12:50Z"), Some(1_794_751_970.0));
        let precise = parse_timestamp("2026-11-15T15:09:44.979Z").unwrap();
        assert!((precise - 1_794_755_384.979).abs() < 1e-6);
        assert_eq!(parse_timestamp("2026-11-15 15:09:44Z"), None);
        assert_eq!(parse_timestamp("2026-13-15T15:09:44Z"), None);
        assert_eq!(parse_timestamp("2026-11-15T15:09:44"), None);
    }

    #[test]
    fn knows_a_twitch_vods_and_streams_chat_and_nothing_elses() {
        assert_eq!(
            ChatSource::of(Some("TwitchVod"), Some("v2929613567"), Some("Greekgodx")),
            Some(ChatSource::Vod {
                id: "2929613567".into(),
                channel: "greekgodx".into()
            })
        );
        assert_eq!(
            ChatSource::of(Some("TwitchStream"), Some("318369725026"), Some("auronplay")),
            Some(ChatSource::Live {
                channel: "auronplay".into()
            })
        );
        assert_eq!(ChatSource::of(Some("TwitchClips"), Some("abc"), Some("x")), None);
        assert_eq!(
            ChatSource::of(Some("Youtube"), Some("jNQXAC9IVRw"), Some("jawed")),
            None
        );
        assert_eq!(ChatSource::of(Some("TwitchVod"), Some("vabc"), Some("x")), None);
        assert_eq!(ChatSource::of(Some("TwitchVod"), Some("v1"), None), None);
        assert_eq!(
            ChatSource::of_channel_link("https://www.twitch.tv/KaiCenat?sr=a"),
            Some(ChatSource::Live {
                channel: "kaicenat".into()
            })
        );
        assert_eq!(ChatSource::of_channel_link("https://www.twitch.tv/videos/123"), None);
        assert_eq!(
            ChatSource::of_channel_link("https://www.twitch.tv/kaicenat/videos"),
            None
        );
        assert_eq!(ChatSource::of_channel_link("https://kick.com/xqc"), None);
    }

    #[test]
    fn a_placement_stays_inside_the_frame() {
        let wild = Placement {
            x: 0.95,
            y: -0.2,
            width: 0.5,
            height: 2.0,
            shade: 7.0,
        }
        .sanitized();
        assert_eq!(wild.width, 0.5);
        assert!((wild.x - 0.5).abs() < 1e-9, "moved back in: {wild:?}");
        assert_eq!((wild.y, wild.height, wild.shade), (0.0, 1.0, 1.0));
        let broken = Placement {
            x: f64::NAN,
            y: 0.1,
            width: 0.0,
            height: f64::INFINITY,
            shade: 0.2,
        }
        .sanitized();
        assert_eq!(broken.width, Placement::MIN_WIDTH);
        assert!(broken.x.is_finite() && broken.height <= 1.0);
        assert_eq!(Placement::DEFAULT.sanitized(), Placement::DEFAULT);
    }

    #[tokio::test(start_paused = true)]
    async fn tries_a_request_again_while_its_failure_may_pass() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        // One request a connection, answered with these in turn.
        let server = tokio::spawn(async move {
            for status in [
                "503 Service Unavailable",
                "429 Too Many Requests",
                "200 OK",
                "404 Not Found",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 8192];
                let _ = socket.read(&mut request).await.unwrap();
                let answer = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let answered = send(|| http.get(&url)).await.unwrap();
        assert_eq!(answered.status(), 200, "after a server error and too many requests");
        // Tried again, it would find the server gone and fail.
        let refused = send(|| http.get(&url)).await.unwrap();
        assert_eq!(refused.status(), 404, "a refusal isn't tried again");
        server.await.unwrap();
    }

    #[test]
    fn reads_a_chat_kept_by_an_older_version() {
        // As 0.16.0 kept it, with the channel and each emote's service and id.
        let old = r##"{"channel":{"id":"1","login":"a","name":"A"},
            "messages":[{"id":"m","time":1.5,"name":"A","login":"a","color":"#FF0000","badges":[],"parts":[{"kind":"text","text":"hi"}],"notice":false}],
            "emotes":[{"name":"KEKW","provider":"seven_tv","id":"x","images":[{"height":32,"url":"https://cdn.example/x"}],"overlay":false}],
            "badges":[]}"##;
        let log: ChatLog = serde_json::from_str(old).unwrap();
        assert_eq!(log.messages.len(), 1);
        assert_eq!(log.emotes[0].name, "KEKW");
    }

    #[test]
    fn picks_the_smallest_image_that_needs_no_enlarging() {
        let images = twitch_emote_images("25");
        assert_eq!(image_for(&images, 20.0).unwrap().height, 28);
        // A tenth larger is close enough.
        assert_eq!(image_for(&images, 30.0).unwrap().height, 28);
        assert_eq!(image_for(&images, 35.0).unwrap().height, 56);
        assert_eq!(image_for(&images, 70.0).unwrap().height, 112);
        assert_eq!(image_for(&images, 400.0).unwrap().height, 112);
        assert!(image_for(&[], 20.0).is_none());
    }
}

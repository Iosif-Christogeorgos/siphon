//! The chat box on the preview: the chat at one moment of the video, drawn
//! by the same layer as the file (see [`super::render`]), so what the
//! preview shows is what the file gets.
//!
//! What a preview needs is kept while its view asks for frames, and let go
//! once it stops: a video's chat and its layer's images take memory, and a
//! live channel's chat is read over a connection of its own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tokio::sync::Mutex;

use super::render::{Geometry, Layer, Style};
use super::twitch::Twitch;
use super::{images, ChatLog, ChatSource, Message, Placement};
use crate::error::Result;
use crate::log;

/// The preview shows the chat from this long before the moment shown…
const BEFORE: f64 = 60.0;
/// …and keeps this much after it, for playing on.
const AFTER: f64 = 30.0;
/// The chat is fetched this far either side of a moment it doesn't have yet.
const FETCH_AROUND: f64 = 90.0;
/// A preview frame is never larger than 4K.
const MAX_SIDE: u32 = 3840;
/// A video's preview is let go once it hasn't been asked for in this long:
/// its view closed, or was left paused (asked again, it fetches its chat again).
const VOD_IDLE: Duration = Duration::from_secs(5 * 60);
/// A live channel's view asks for its box every second, so one not asked for
/// in this long has closed, and its chat stops being read.
const LIVE_IDLE: Duration = Duration::from_secs(15);
/// How often the previews are looked at for ones to let go of.
const SWEEP_EVERY: Duration = Duration::from_secs(5);
/// The newest messages a live preview keeps: more than a box can show.
const LIVE_KEEP: usize = 300;

/// What the previews need of each video: its chat as far as fetched, and a
/// layer with its fonts and images loaded, kept between frames. A live
/// channel's chat is read as it comes, while its preview is asked for.
#[derive(Default)]
pub struct ChatPreviews {
    kept: Arc<Kept>,
    /// Lets go of the previews nothing asks for any more; started with the first.
    sweeper: OnceLock<()>,
}

#[derive(Default)]
struct Kept {
    vods: Mutex<HashMap<String, Preview>>,
    channels: Mutex<HashMap<String, LivePreview>>,
}

/// A live channel's chat for the preview: read as it comes, newest last.
struct LivePreview {
    twitch: Twitch,
    log: ChatLog,
    recent: Arc<std::sync::Mutex<VecDeque<Message>>>,
    reader: tokio::task::JoinHandle<()>,
    last_asked: Instant,
    layer: Option<Drawing>,
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

struct Preview {
    twitch: Twitch,
    log: ChatLog,
    ids: HashSet<String>,
    /// Stretches of the video whose chat has been fetched, in seconds.
    fetched: Vec<(f64, f64)>,
    layer: Option<Drawing>,
    last_asked: Instant,
}

/// A layer, and what it was made for.
struct Drawing {
    placement: Placement,
    width: u32,
    height: u32,
    layer: Layer,
    /// Where the stretch of the chat the layer has ends, and the moment it last drew.
    until: f64,
    last: f64,
}

/// One frame of the chat box: its place on the frame, and its pixels
/// (straight RGBA, the box's size).
pub struct Frame {
    pub geometry: Geometry,
    pub pixels: Vec<u8>,
}

impl ChatPreviews {
    /// The chat box of `source` at `at` seconds, placed by `placement` on a
    /// frame `width` × `height` (the preview's size, so it's drawn sharp).
    #[allow(clippy::too_many_arguments)]
    pub async fn frame(
        &self,
        app: &AppHandle,
        source: &ChatSource,
        images_dir: PathBuf,
        at: f64,
        placement: Placement,
        width: u32,
        height: u32,
    ) -> Result<Frame> {
        self.start_sweeping(app);
        let placement = placement.sanitized();
        let (width, height) = (width.clamp(2, MAX_SIDE), height.clamp(2, MAX_SIDE));
        let id = match source {
            ChatSource::Vod { id, .. } => id,
            ChatSource::Live { channel } => {
                return self.live_frame(channel, images_dir, placement, width, height).await;
            }
        };
        let at = at.max(0.0);
        let mut previews = self.kept.vods.lock().await;
        if !previews.contains_key(id) {
            let preview = Preview::start(id).await?;
            previews.insert(id.clone(), preview);
        }
        let preview = previews.get_mut(id).expect("just added");
        preview.last_asked = Instant::now();
        let fresh = preview.fetch_around(id, at).await?;

        // The images this stretch shows, at the sizes this frame draws them.
        let geometry = Geometry::new(&placement, width, height);
        let style = Style::for_width(geometry.width);
        let urls = preview.log.images_used(at - BEFORE, at, style.emote(), style.badge());
        images::fetch(preview.twitch.http(), &images_dir, urls).await;

        let reuse = preview.layer.as_ref().is_some_and(|drawing| {
            drawing.placement == placement && drawing.width == width && drawing.height == height
        });
        if !reuse {
            let log = ChatLog {
                messages: Vec::new(),
                ..preview.log.clone()
            };
            preview.layer = Some(Drawing {
                placement,
                width,
                height,
                layer: Layer::new(log, &placement, width, height, &images_dir),
                until: f64::NAN,
                last: f64::NAN,
            });
        }
        let messages = window(&preview.log.messages, at);
        let drawing = preview.layer.as_mut().expect("just made");
        // Playing on from the last frame carries on; a jump, or chat that
        // arrived meanwhile, starts the layer over around the new moment.
        let carries_on = !fresh && at >= drawing.last && at <= drawing.until - 5.0 && at - drawing.last < 5.0;
        if !carries_on {
            drawing.layer.restart(messages);
            drawing.until = at + AFTER;
        }
        drawing.last = at;
        let frame = Frame {
            pixels: drawing.layer.frame(at).to_vec(),
            geometry: drawing.layer.geometry(),
        };
        // Asked for until now: fetching the chat may have taken a while.
        preview.last_asked = Instant::now();
        Ok(frame)
    }

    /// Starts letting go of the previews nothing asks for any more, the
    /// first time a preview is asked for.
    fn start_sweeping(&self, app: &AppHandle) {
        self.sweeper.get_or_init(|| {
            let (app, kept) = (app.clone(), Arc::clone(&self.kept));
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(SWEEP_EVERY).await;
                    for name in kept.let_go(Instant::now()).await {
                        log::line(
                            &app,
                            &format!("chat preview of {name}: let go, nothing shows it any more"),
                        );
                    }
                }
            });
        });
    }
}

impl Kept {
    /// Lets go of the previews not asked for lately, as of `now`: a live
    /// channel's chat stops being read. Says which.
    async fn let_go(&self, now: Instant) -> Vec<String> {
        let idle = |asked: Instant, limit: Duration| now.saturating_duration_since(asked) >= limit;
        let mut gone = Vec::new();
        self.channels.lock().await.retain(|channel, preview| {
            let stale = idle(preview.last_asked, LIVE_IDLE);
            if stale {
                gone.push(format!("#{channel}"));
            }
            !stale
        });
        self.vods.lock().await.retain(|id, preview| {
            let stale = idle(preview.last_asked, VOD_IDLE);
            if stale {
                gone.push(format!("VOD {id}"));
            }
            !stale
        });
        gone
    }
}

impl ChatPreviews {
    /// The chat box of live `channel` as it is now, placed by `placement` on
    /// a frame `width` × `height`. The first ask starts reading its chat.
    async fn live_frame(
        &self,
        channel: &str,
        images_dir: PathBuf,
        placement: Placement,
        width: u32,
        height: u32,
    ) -> Result<Frame> {
        let mut channels = self.kept.channels.lock().await;
        if !channels.contains_key(channel) {
            let preview = LivePreview::start(channel).await?;
            channels.insert(channel.to_string(), preview);
        }
        let preview = channels.get_mut(channel).expect("just added");
        preview.last_asked = Instant::now();
        let messages: Vec<Message> = preview.recent.lock().unwrap().iter().cloned().collect();
        let now = messages.last().map_or(0.0, |message| message.time) + 0.001;

        let geometry = Geometry::new(&placement, width, height);
        let style = Style::for_width(geometry.width);
        let log = ChatLog {
            messages: messages.clone(),
            ..preview.log.clone()
        };
        images::fetch(
            preview.twitch.http(),
            &images_dir,
            log.images_used(0.0, f64::MAX, style.emote(), style.badge()),
        )
        .await;
        let reuse = preview.layer.as_ref().is_some_and(|drawing| {
            drawing.placement == placement && drawing.width == width && drawing.height == height
        });
        if !reuse {
            preview.layer = Some(Drawing {
                placement,
                width,
                height,
                layer: Layer::new(
                    ChatLog {
                        messages: Vec::new(),
                        ..log
                    },
                    &placement,
                    width,
                    height,
                    &images_dir,
                ),
                until: f64::NAN,
                last: f64::NAN,
            });
        }
        let drawing = preview.layer.as_mut().expect("just made");
        drawing.layer.restart(messages);
        let frame = Frame {
            pixels: drawing.layer.frame(now).to_vec(),
            geometry: drawing.layer.geometry(),
        };
        // Asked for until now: its images may have taken a while.
        preview.last_asked = Instant::now();
        Ok(frame)
    }
}

impl LivePreview {
    async fn start(login: &str) -> Result<LivePreview> {
        let twitch = Twitch::new()?;
        let channel_id = twitch.channel_id(login).await?;
        let (log, _) = ChatLog::of_channel(&twitch, &channel_id).await;
        let recent = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let (into, login) = (Arc::clone(&recent), login.to_string());
        let reader = tokio::spawn(async move {
            super::irc::listen(
                &login,
                |event| {
                    let mut recent = into.lock().unwrap();
                    match event {
                        super::irc::Event::Message(message) => {
                            recent.push_back(message);
                            while recent.len() > LIVE_KEEP {
                                recent.pop_front();
                            }
                        }
                        super::irc::Event::Delete { id } => recent.retain(|message| message.id != id),
                        super::irc::Event::ClearUser { login, .. } => recent.retain(|message| message.login != login),
                    }
                },
                |_| {},
            )
            .await;
        });
        Ok(LivePreview {
            twitch,
            log,
            recent,
            reader,
            last_asked: Instant::now(),
            layer: None,
        })
    }
}

/// The messages a preview at `at` needs: the ones that may still be in the
/// box, and those coming up next.
fn window(messages: &[Message], at: f64) -> Vec<Message> {
    let start = messages.partition_point(|message| message.time < at - BEFORE);
    let end = messages.partition_point(|message| message.time <= at + AFTER);
    messages[start..end].to_vec()
}

impl Preview {
    async fn start(id: &str) -> Result<Preview> {
        let twitch = Twitch::new()?;
        let vod = twitch.vod(id).await?;
        let (log, _) = ChatLog::of_channel(&twitch, &vod.channel_id).await;
        Ok(Preview {
            twitch,
            log,
            ids: HashSet::new(),
            fetched: Vec::new(),
            layer: None,
            last_asked: Instant::now(),
        })
    }

    /// Fetches the chat around `at` if it hasn't been. True if any arrived.
    async fn fetch_around(&mut self, id: &str, at: f64) -> Result<bool> {
        let (from, to) = ((at - BEFORE).max(0.0), at + AFTER);
        if self.fetched.iter().any(|(start, end)| *start <= from && *end >= to) {
            return Ok(false);
        }
        let (start, end) = ((at - FETCH_AROUND).max(0.0), at + FETCH_AROUND);
        let replay = self.twitch.comments(id, start, end, |_| {}).await?;
        self.fetched.push((start, end));
        let before = self.log.messages.len();
        for message in replay.messages {
            if self.ids.insert(message.id.clone()) {
                self.log.messages.push(message);
            }
        }
        self.log.messages.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(self.log.messages.len() > before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Part;

    #[test]
    fn a_preview_keeps_the_chat_around_its_moment() {
        let messages: Vec<Message> = [10.0, 100.0, 150.0, 170.0, 200.0, 300.0]
            .into_iter()
            .map(|time| Message {
                id: time.to_string(),
                time,
                name: "a".into(),
                login: "a".into(),
                color: None,
                badges: Vec::new(),
                parts: vec![Part::Text { text: "hi".into() }],
                notice: false,
            })
            .collect();
        let times: Vec<f64> = window(&messages, 160.0).iter().map(|m| m.time).collect();
        assert_eq!(times, vec![100.0, 150.0, 170.0]);
    }

    #[tokio::test]
    async fn lets_go_of_previews_nothing_asks_for_and_stops_reading_their_chat() {
        let asked = Instant::now();
        let kept = Kept::default();
        kept.vods.lock().await.insert(
            "1".into(),
            Preview {
                twitch: Twitch::new().unwrap(),
                log: ChatLog::default(),
                ids: HashSet::new(),
                fetched: Vec::new(),
                layer: None,
                last_asked: asked,
            },
        );
        // The chat reader: its end drops `reading`, which `ended` hears.
        let (reading, ended) = tokio::sync::oneshot::channel::<()>();
        let reader = tokio::spawn(async move {
            let _reading = reading;
            std::future::pending::<()>().await;
        });
        kept.channels.lock().await.insert(
            "somebody".into(),
            LivePreview {
                twitch: Twitch::new().unwrap(),
                log: ChatLog::default(),
                recent: Arc::default(),
                reader,
                last_asked: asked,
                layer: None,
            },
        );

        assert!(
            kept.let_go(asked + Duration::from_secs(5)).await.is_empty(),
            "both still shown"
        );
        // The live view asks every second: quiet this long, it has closed.
        assert_eq!(kept.let_go(asked + LIVE_IDLE).await, vec!["#somebody"]);
        let stopped = tokio::time::timeout(Duration::from_secs(5), ended).await;
        assert!(matches!(stopped, Ok(Err(_))), "its chat stopped being read");
        assert_eq!(
            kept.vods.lock().await.len(),
            1,
            "a paused video's is kept a while longer"
        );
        assert_eq!(kept.let_go(asked + VOD_IDLE).await, vec!["VOD 1"]);
        assert!(kept.vods.lock().await.is_empty());
    }
}

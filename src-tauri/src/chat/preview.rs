//! The chat box on the preview: the chat at one moment of the video, drawn
//! by the same layer as the file (see [`super::render`]), so what the
//! preview shows is what the file gets.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::render::{Geometry, Layer, Style};
use super::twitch::Twitch;
use super::{emotes, images, ChatLog, ChatSource, Message, Placement};
use crate::error::Result;

/// The preview shows the chat from this long before the moment shown…
const BEFORE: f64 = 60.0;
/// …and keeps this much after it, for playing on.
const AFTER: f64 = 30.0;
/// The chat is fetched this far either side of a moment it doesn't have yet.
const FETCH_AROUND: f64 = 90.0;
/// A preview frame is never larger than 4K.
const MAX_SIDE: u32 = 3840;

/// What the preview needs of each video: its chat as far as fetched, and a
/// layer with its fonts and images loaded, kept between frames. A live
/// channel's chat is read as it comes, while its preview is asked for.
#[derive(Default)]
pub struct ChatPreviews {
    vods: tokio::sync::Mutex<HashMap<String, Preview>>,
    channels: tokio::sync::Mutex<HashMap<String, LivePreview>>,
}

/// A live preview stops reading its channel's chat once it hasn't been
/// asked for in this long: the window closed, or the video changed.
const LIVE_IDLE: std::time::Duration = std::time::Duration::from_secs(90);
/// The newest messages a live preview keeps: more than a box can show.
const LIVE_KEEP: usize = 300;

/// A live channel's chat for the preview: read as it comes, newest last.
struct LivePreview {
    twitch: Twitch,
    log: ChatLog,
    recent: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Message>>>,
    reader: tokio::task::JoinHandle<()>,
    last_asked: std::time::Instant,
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
    pub async fn frame(
        &self,
        source: &ChatSource,
        images_dir: PathBuf,
        at: f64,
        placement: Placement,
        width: u32,
        height: u32,
    ) -> Result<Frame> {
        let placement = placement.sanitized();
        let (width, height) = (width.clamp(2, MAX_SIDE), height.clamp(2, MAX_SIDE));
        let id = match source {
            ChatSource::Vod { id, .. } => id,
            ChatSource::Live { channel } => {
                return self.live_frame(channel, images_dir, placement, width, height).await;
            }
        };
        let at = at.max(0.0);
        let mut previews = self.vods.lock().await;
        if !previews.contains_key(id) {
            let preview = Preview::start(id).await?;
            previews.insert(id.clone(), preview);
        }
        let preview = previews.get_mut(id).expect("just added");
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
        let pixels = drawing.layer.frame(at).to_vec();
        Ok(Frame {
            geometry: drawing.layer.geometry(),
            pixels,
        })
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
        let mut channels = self.channels.lock().await;
        // Channels no window has asked about for a while stop being read.
        channels.retain(|_, preview| preview.last_asked.elapsed() < LIVE_IDLE);
        if !channels.contains_key(channel) {
            let preview = LivePreview::start(channel).await?;
            channels.insert(channel.to_string(), preview);
        }
        let preview = channels.get_mut(channel).expect("just added");
        preview.last_asked = std::time::Instant::now();
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
        Ok(Frame {
            pixels: drawing.layer.frame(now).to_vec(),
            geometry: drawing.layer.geometry(),
        })
    }
}

impl LivePreview {
    async fn start(login: &str) -> Result<LivePreview> {
        let twitch = Twitch::new()?;
        let channel = twitch.channel(login).await?;
        let ((emote_list, _), badges) =
            tokio::join!(emotes::fetch(twitch.http(), &channel.id), twitch.badges(&channel.id));
        let recent = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        let (into, login) = (std::sync::Arc::clone(&recent), login.to_string());
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
            log: ChatLog {
                channel,
                messages: Vec::new(),
                emotes: emote_list,
                badges: badges.unwrap_or_default(),
            },
            recent,
            reader,
            last_asked: std::time::Instant::now(),
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
        let ((emote_list, _), badges) = tokio::join!(
            emotes::fetch(twitch.http(), &vod.channel.id),
            twitch.badges(&vod.channel.id)
        );
        Ok(Preview {
            twitch,
            log: ChatLog {
                channel: vod.channel,
                messages: Vec::new(),
                emotes: emote_list,
                badges: badges.unwrap_or_default(),
            },
            ids: HashSet::new(),
            fetched: Vec::new(),
            layer: None,
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
}

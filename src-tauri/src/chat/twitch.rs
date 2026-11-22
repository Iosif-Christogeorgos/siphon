//! Twitch's GraphQL API: a VOD's owner and chat replay, a channel's badges.
//!
//! Read without an account, with the public client ID of Twitch's own web
//! player (yt-dlp uses it too). The chat replay can only be paged by time:
//! paging by cursor fails Twitch's "integrity check" without a browser
//! token. A page asked for at a time holds a run of about fifty comments
//! starting a little before it, and every time within the same three
//! seconds gives the same page; Twitch keeps at most 15 comments a second
//! (all measured 2026-11-21, docs/chat.md). So a stretch of the video is
//! covered by pages that overlap, found by probing every 30 seconds and
//! filling in between wherever two neighbouring pages don't overlap.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::{Badge, BadgeRef, Image, Message, Part};
use crate::error::{AppError, ErrorKind, Result};

const GQL: &str = "https://gql.twitch.tv/gql";
/// The client ID of Twitch's web player, which anyone can read in its page.
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
/// Twitch's stored query for a page of a video's chat.
const COMMENTS_QUERY: &str = "b70a3591ff0f4e0313d126c6a1502d79a1c02baebb288227c582044aa76adf6a";
/// Operations per request: Twitch refuses more ("BATCH_LIMIT_EXCEEDED").
/// It answers a request's operations one after another, about 0.15 s each
/// (2026-11-21), so a round's pages go in several smaller requests at once.
const BATCH: usize = 35;
/// The fewest operations worth a request of their own.
const SMALL_BATCH: usize = 4;
/// Requests in flight at once, for all of a download's chat together.
const PARALLEL: usize = 8;
/// Every time in a bucket of this many seconds gives the same page.
const BUCKET: u32 = 3;
/// Each stretch is first probed this often, then filled in where needed.
const PROBE: u32 = 30;
/// The video is fetched in stretches of this length, several at once.
const STRETCH: u32 = 600;

pub struct Twitch {
    http: reqwest::Client,
}

/// A stretch of a VOD's chat replay, each message at its own moment.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    pub messages: Vec<Message>,
    /// Places where two pages a bucket apart didn't meet, so some comments
    /// between them may be missing. Logged; Twitch has no page between them.
    pub gaps: usize,
    /// When the video starts as its replay counts, in seconds since 1970,
    /// on the clock of IRC's `tmi-sent-ts` (see [`timed`]). `None` without a
    /// comment that says when it was sent.
    pub origin: Option<f64>,
}

/// A VOD: whose it is, and how long.
#[derive(Debug, Clone, PartialEq)]
pub struct Vod {
    /// Twitch's number for the channel, which its badges and the emote services key by.
    pub channel_id: String,
    pub length: f64,
}

/// A comment as a page gives it, before its time is worked out.
#[derive(Debug, Clone, PartialEq)]
struct Comment {
    id: String,
    /// Whole seconds into the video, as Twitch counts them.
    offset: u32,
    /// When it was sent, in seconds since 1970, to the millisecond.
    sent: Option<f64>,
    name: String,
    login: String,
    color: Option<String>,
    badges: Vec<BadgeRef>,
    parts: Vec<Part>,
}

/// One page of comments, asked for at `bucket` seconds.
#[derive(Debug, Clone, Default)]
struct Page {
    comments: Vec<Comment>,
    /// Twitch says there are comments after this page.
    more: bool,
}

impl Page {
    fn first(&self) -> Option<u32> {
        self.comments.iter().map(|c| c.offset).min()
    }

    fn last(&self) -> Option<u32> {
        self.comments.iter().map(|c| c.offset).max()
    }
}

impl Twitch {
    pub fn new() -> Result<Self> {
        let http = crate::throttle::follow_system_proxy(reqwest::Client::builder())
            .user_agent(concat!("Siphon/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|err| {
                AppError::internal("Couldn't set up a connection to Twitch.").with_details(err.to_string())
            })?;
        Ok(Self { http })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Sends `body` (one operation, or a list of them), trying again twice
    /// after a dropped connection, a server error or "too many requests".
    async fn post(&self, body: &Value) -> Result<Value> {
        let mut last = None;
        for delay in [0, 2, 6] {
            if delay > 0 {
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
            let sent = self
                .http
                .post(GQL)
                .header("Client-ID", CLIENT_ID)
                .header("Content-Type", "text/plain;charset=UTF-8")
                .body(body.to_string())
                .send()
                .await;
            match sent {
                Ok(response) if response.status().is_success() => {
                    return response
                        .json::<Value>()
                        .await
                        .map_err(|err| refused("Twitch's answer couldn't be read.").with_details(err.to_string()));
                }
                Ok(response) => {
                    let status = response.status();
                    let passing = status.is_server_error() || status.as_u16() == 429;
                    last =
                        Some(refused("Twitch turned the request for the chat down.").with_details(status.to_string()));
                    if !passing {
                        break;
                    }
                }
                Err(err) => last = Some(AppError::from(err)),
            }
        }
        Err(last.unwrap_or_else(|| refused("Twitch didn't answer.")))
    }

    /// One query, with its variables; the data it answered, or why not.
    async fn query(&self, query: &str, variables: Value) -> Result<Value> {
        let answer = self.post(&json!({ "query": query, "variables": variables })).await?;
        if let Some(message) = first_error(&answer) {
            return Err(refused("Twitch didn't give the chat.").with_details(message));
        }
        Ok(answer.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Whose VOD `id` is (a number, without yt-dlp's "v" in front) and how long it is.
    pub async fn vod(&self, id: &str) -> Result<Vod> {
        let data = self
            .query(
                "query($id:ID!){video(id:$id){lengthSeconds owner{id}}}",
                json!({ "id": id }),
            )
            .await?;
        parse_vod(&data).ok_or_else(|| refused("Twitch doesn't have this video's chat."))
    }

    /// Twitch's number for the channel with the login `login`.
    pub async fn channel_id(&self, login: &str) -> Result<String> {
        let data = self
            .query(
                "query($login:String!){user(login:$login){id}}",
                json!({ "login": login }),
            )
            .await?;
        user_id(data.get("user")).ok_or_else(|| refused("Twitch has no such channel."))
    }

    /// Everyone's badges and the channel's own (its subscriber and bits
    /// badges), the channel's last so they win where both have one.
    pub async fn badges(&self, channel_id: &str) -> Result<Vec<Badge>> {
        let data = self
            .query(
                "query($id:ID!){badges{setID version imageURL(size:DOUBLE)} \
                 user(id:$id){broadcastBadges{setID version imageURL(size:DOUBLE)}}}",
                json!({ "id": channel_id }),
            )
            .await?;
        let mut badges = parse_badges(data.get("badges"));
        badges.extend(parse_badges(data.pointer("/user/broadcastBadges")));
        Ok(badges)
    }

    /// The chat of VOD `id` between `from` and `to` seconds, in time order.
    /// `progress` hears how much is done (0–1).
    pub async fn comments(&self, id: &str, from: f64, to: f64, progress: impl Fn(f64) + Send + Sync) -> Result<Replay> {
        let from = from.max(0.0).floor() as u32;
        let to = to.max(0.0).ceil() as u32;
        let stretches: Vec<(u32, u32)> = (from / STRETCH * STRETCH..=to)
            .step_by(STRETCH as usize)
            .map(|start| (start.max(from), (start + STRETCH).min(to)))
            .filter(|(start, end)| start < end)
            .collect();
        let total = stretches.len().max(1);
        let permits = Arc::new(Semaphore::new(PARALLEL));
        let mut done = 0;
        let mut all: HashMap<String, Comment> = HashMap::new();
        let mut gaps = 0;
        let mut walks = futures_util::stream::iter(stretches)
            .map(|(start, end)| self.walk(id, start, end, Arc::clone(&permits)))
            .buffer_unordered(PARALLEL * 2);
        while let Some(walked) = walks.next().await {
            let (comments, unfilled) = walked?;
            gaps += unfilled;
            for comment in comments {
                all.entry(comment.id.clone()).or_insert(comment);
            }
            done += 1;
            progress(done as f64 / total as f64);
        }
        let comments: Vec<Comment> = all
            .into_values()
            .filter(|comment| comment.offset >= from && comment.offset <= to)
            .collect();
        Ok(Replay {
            origin: origin(&comments),
            messages: timed(comments),
            gaps,
        })
    }

    /// Covers `start` to `end` seconds with pages that overlap: a probe
    /// every 30 seconds, then pages in between wherever two neighbours
    /// don't meet. Returns every comment seen, and how many gaps couldn't be
    /// filled (two pages a bucket apart that still don't meet).
    async fn walk(&self, id: &str, start: u32, end: u32, permits: Arc<Semaphore>) -> Result<(Vec<Comment>, usize)> {
        let mut pages: BTreeMap<u32, Page> = BTreeMap::new();
        let mut wanted: Vec<u32> = (bucket(start)..=bucket(end)).step_by(PROBE as usize).collect();
        // The end too, so this stretch meets the next one.
        wanted.push(bucket(end));
        let mut unfilled = 0;
        while !wanted.is_empty() {
            wanted.sort_unstable();
            wanted.dedup();
            wanted.retain(|bucket| !pages.contains_key(bucket));
            // Spread over every request that may run at once, no fewer than a few each.
            let size = wanted.len().div_ceil(PARALLEL).clamp(SMALL_BATCH, BATCH);
            let requests = wanted.chunks(size).map(|chunk| {
                let permits = Arc::clone(&permits);
                async move {
                    let _permit = permits.acquire().await.map_err(|_| AppError::cancelled())?;
                    let fetched = self.pages(id, chunk).await?;
                    Ok::<_, AppError>(chunk.iter().copied().zip(fetched).collect::<Vec<_>>())
                }
            });
            for fetched in futures_util::future::try_join_all(requests).await? {
                pages.extend(fetched);
            }
            let (more, stuck) = fill_in(&pages);
            unfilled = stuck;
            wanted = more;
        }
        let comments = pages.into_values().flat_map(|page| page.comments).collect();
        Ok((comments, unfilled))
    }

    /// The pages at `buckets`, in one request.
    async fn pages(&self, id: &str, buckets: &[u32]) -> Result<Vec<Page>> {
        let body: Vec<Value> = buckets
            .iter()
            .map(|offset| {
                json!({
                    "operationName": "VideoCommentsByOffsetOrCursor",
                    "variables": { "videoID": id, "contentOffsetSeconds": offset },
                    "extensions": { "persistedQuery": { "version": 1, "sha256Hash": COMMENTS_QUERY } },
                })
            })
            .collect();
        let answer = self.post(&Value::Array(body)).await?;
        let results = answer.as_array().cloned().unwrap_or_default();
        if results.len() != buckets.len() {
            return Err(refused("Twitch's answer about the chat was cut short.").with_details(answer.to_string()));
        }
        results.iter().map(parse_page).collect()
    }
}

/// Where the next pages should be asked for, so that neighbouring pages
/// overlap; and how many gaps can't be filled, because the pages either side
/// are a bucket apart. For a pair that doesn't meet, the next page starts
/// where the first left off, and the space after it is probed at the
/// distance the first one covered, since chat that busy likely goes on.
fn fill_in(pages: &BTreeMap<u32, Page>) -> (Vec<u32>, usize) {
    let mut wanted = Vec::new();
    let mut stuck = 0;
    let fetched: Vec<(&u32, &Page)> = pages.iter().collect();
    for pair in fetched.windows(2) {
        let ((&a_bucket, a), (&b_bucket, b)) = (pair[0], pair[1]);
        if meets(a, b, b_bucket) {
            continue;
        }
        let Some(last) = a.last() else { continue };
        let next = bucket(last).max(a_bucket + BUCKET);
        if next >= b_bucket {
            stuck += 1;
            continue;
        }
        let step = (next - a_bucket).max(BUCKET);
        wanted.extend((next..b_bucket).step_by(step as usize));
    }
    (wanted, stuck)
}

/// Whether page `a` and the next page fetched, `b` (asked for at
/// `b_bucket`), leave no comment out between them. Each page is an unbroken
/// run of comments in time order, so they meet when they share a comment or
/// `b` starts before `a` ends.
fn meets(a: &Page, b: &Page, b_bucket: u32) -> bool {
    let Some(a_last) = a.last() else {
        // Nothing at or after `a`'s time at all.
        return true;
    };
    if !a.more {
        // `a` runs to the end of the chat.
        return true;
    }
    if a.comments.iter().any(|c| b.comments.iter().any(|d| d.id == c.id)) {
        return true;
    }
    match b.first() {
        Some(b_first) => b_first < a_last,
        // Nothing at or after `b`'s time: `a` has to reach it.
        None => a_last >= b_bucket,
    }
}

fn bucket(seconds: u32) -> u32 {
    seconds - seconds % BUCKET
}

/// The video's start, as Twitch counts it, in seconds since 1970: the
/// earliest "sent minus seconds in" over all comments. Twitch gives whole
/// seconds into the video, rounded down, and the moment each was sent.
fn origin(comments: &[Comment]) -> Option<f64> {
    let start = comments
        .iter()
        .filter_map(|c| c.sent.map(|sent| sent - f64::from(c.offset)))
        .fold(f64::INFINITY, f64::min);
    start.is_finite().then_some(start)
}

/// Each comment's moment, to the millisecond: when it was sent, less the
/// video's [`origin`], which places every comment within its second.
fn timed(comments: Vec<Comment>) -> Vec<Message> {
    let start = origin(&comments);
    let mut messages: Vec<Message> = comments
        .into_iter()
        .map(|c| {
            let whole = f64::from(c.offset);
            let time = match (c.sent, start) {
                (Some(sent), Some(start)) => (sent - start).clamp(whole, whole + 0.999),
                _ => whole,
            };
            Message {
                id: c.id,
                time,
                name: c.name,
                login: c.login,
                color: c.color,
                badges: c.badges,
                parts: c.parts,
                notice: false,
            }
        })
        .collect();
    messages.sort_by(|a, b| a.time.total_cmp(&b.time).then_with(|| a.id.cmp(&b.id)));
    messages
}

fn refused(message: &str) -> AppError {
    AppError::new(ErrorKind::Unavailable, message)
}

fn first_error(answer: &Value) -> Option<String> {
    let errors = answer.get("errors")?.as_array()?;
    let first = errors.first()?;
    Some(
        first
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    )
}

fn parse_vod(data: &Value) -> Option<Vod> {
    let video = data.get("video")?;
    Some(Vod {
        channel_id: user_id(video.get("owner"))?,
        length: video.get("lengthSeconds")?.as_f64()?,
    })
}

fn user_id(user: Option<&Value>) -> Option<String> {
    user?
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn parse_badges(list: Option<&Value>) -> Vec<Badge> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|badge| {
            let text = |key: &str| badge.get(key).and_then(Value::as_str);
            // ".../badges/v1/<id>/2" is 36 pixels high; /1 is 18 and /3 is 72.
            let url = text("imageURL")?;
            let base = url.strip_suffix("/2").unwrap_or(url);
            Some(Badge {
                set: text("setID")?.to_string(),
                version: text("version")?.to_string(),
                images: [(18, "1"), (36, "2"), (72, "3")]
                    .into_iter()
                    .map(|(height, size)| Image {
                        height,
                        url: format!("{base}/{size}"),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// One page of a batch: its comments, or why Twitch refused it.
fn parse_page(result: &Value) -> Result<Page> {
    if let Some(message) = first_error(result) {
        let blocked = message.contains("integrity");
        let error = if blocked {
            refused("Twitch didn't let Siphon read this video's chat.")
        } else {
            refused("Twitch didn't give this video's chat.")
        };
        return Err(error.with_details(message));
    }
    let Some(comments) = result.pointer("/data/video/comments").filter(|c| !c.is_null()) else {
        // No such video, or one without a chat replay.
        return Ok(Page::default());
    };
    let edges = comments
        .get("edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Page {
        comments: edges
            .iter()
            .filter_map(|edge| parse_comment(edge.get("node")?))
            .collect(),
        more: comments
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

fn parse_comment(node: &Value) -> Option<Comment> {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    // A comment by an account that has since been deleted has no commenter: Twitch shows none either.
    let commenter = node.get("commenter").filter(|c| !c.is_null())?;
    let login = text(commenter, "login")?;
    let message = node.get("message")?;
    let parts = message
        .get("fragments")
        .and_then(Value::as_array)
        .map(|fragments| {
            fragments
                .iter()
                .filter_map(|fragment| {
                    let words = text(fragment, "text")?;
                    match fragment.get("emote").and_then(|emote| text(emote, "emoteID")) {
                        Some(id) => Some(Part::Emote { id, name: words }),
                        None => Some(Part::Text { text: words }),
                    }
                })
                .collect::<Vec<Part>>()
        })
        .unwrap_or_default();
    let badges = message
        .get("userBadges")
        .and_then(Value::as_array)
        .map(|badges| {
            badges
                .iter()
                .filter_map(|badge| {
                    Some(BadgeRef {
                        set: text(badge, "setID")?,
                        version: text(badge, "version")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Comment {
        id: text(node, "id")?,
        offset: node.get("contentOffsetSeconds")?.as_u64()? as u32,
        sent: text(node, "createdAt").and_then(|at| super::parse_timestamp(&at)),
        name: text(commenter, "displayName").unwrap_or_else(|| login.clone()),
        login,
        color: text(message, "userColor").filter(|color| color.starts_with('#') && color.len() == 7),
        badges,
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page as Twitch sent it on 2026-11-21 (two comments, trimmed).
    const PAGE: &str = r##"{"data":{"video":{"id":"2929613567","creator":{"id":"15310631","channel":{"id":"15310631","__typename":"Channel"},"__typename":"User"},"comments":{"edges":[
        {"cursor":"eyJpZCI6IjQ5MzA4MjIyIn0","node":{"id":"404f3ba0-c5b6-4293-93e0-c6b4a33de0e7","commenter":{"id":"155345517","login":"twigsterman","displayName":"TwigsterMan","__typename":"User"},"contentOffsetSeconds":3414,"createdAt":"2026-11-15T15:09:44.979Z","message":{"fragments":[{"emote":null,"text":"i found a meth lab @Greekgodx","__typename":"VideoCommentMessageFragment"}],"userBadges":[{"id":"c3Vic2NyaWJlcjs2Ow==","setID":"subscriber","version":"6","__typename":"Badge"}],"userColor":"#1E2D1D","__typename":"VideoCommentMessage"},"__typename":"VideoComment"},"__typename":"VideoCommentEdge"},
        {"cursor":"eyJpZCI6IjQ5MzA4MjIyIn0","node":{"id":"52786e80-6bd8-4887-80a6-bd214955a3b5","commenter":{"id":"1","login":"rendingturn","displayName":"Rendingturn","__typename":"User"},"contentOffsetSeconds":3436,"createdAt":"2026-11-15T15:10:07.283Z","message":{"fragments":[{"emote":null,"text":"hello ","__typename":"VideoCommentMessageFragment"},{"emote":{"id":"120232;7;13","emoteID":"120232","from":7,"__typename":"EmbeddedEmote"},"text":"TriHard","__typename":"VideoCommentMessageFragment"}],"userBadges":[],"userColor":null,"__typename":"VideoCommentMessage"},"__typename":"VideoComment"},"__typename":"VideoCommentEdge"},
        {"cursor":"x","node":{"id":"gone","commenter":null,"contentOffsetSeconds":3437,"createdAt":"2026-11-15T15:10:08Z","message":{"fragments":[{"emote":null,"text":"from a deleted account"}],"userBadges":[]}}}
    ],"pageInfo":{"hasNextPage":true,"hasPreviousPage":true,"__typename":"PageInfo"},"__typename":"VideoCommentConnection"},"__typename":"Video"}},"extensions":{"durationMilliseconds":34,"operationName":"VideoCommentsByOffsetOrCursor"}}"##;

    fn page(comments: &[(&str, u32)], more: bool) -> Page {
        Page {
            comments: comments
                .iter()
                .map(|(id, offset)| Comment {
                    id: id.to_string(),
                    offset: *offset,
                    sent: None,
                    name: String::new(),
                    login: String::new(),
                    color: None,
                    badges: Vec::new(),
                    parts: Vec::new(),
                })
                .collect(),
            more,
        }
    }

    #[test]
    fn reads_a_page_of_comments_as_twitch_sends_it() {
        let page = parse_page(&serde_json::from_str(PAGE).unwrap()).unwrap();
        assert!(page.more);
        assert_eq!(page.comments.len(), 2, "the deleted account's comment is left out");
        let first = &page.comments[0];
        assert_eq!(
            (first.offset, first.name.as_str(), first.login.as_str()),
            (3414, "TwigsterMan", "twigsterman")
        );
        assert_eq!(first.color.as_deref(), Some("#1E2D1D"));
        assert_eq!(
            first.badges,
            vec![BadgeRef {
                set: "subscriber".into(),
                version: "6".into()
            }]
        );
        assert!((first.sent.unwrap() - 1_794_755_384.979).abs() < 1e-6);
        let second = &page.comments[1];
        assert_eq!(second.color, None);
        assert_eq!(
            second.parts,
            vec![
                Part::Text { text: "hello ".into() },
                Part::Emote {
                    id: "120232".into(),
                    name: "TriHard".into()
                }
            ]
        );
    }

    #[test]
    fn a_refused_page_says_why() {
        let refused: Value = serde_json::from_str(
            r#"{"errors":[{"message":"failed integrity check","path":["video","comments"],"extensions":{"code":"IntegrityCheckFailed"}}],"data":{"video":{"id":"1","comments":null}}}"#,
        )
        .unwrap();
        let error = parse_page(&refused).unwrap_err();
        assert_eq!(error.message, "Twitch didn't let Siphon read this video's chat.");
        // A video without a replay is an empty page, not an error.
        let empty: Value = serde_json::from_str(r#"{"data":{"video":null}}"#).unwrap();
        assert!(parse_page(&empty).unwrap().comments.is_empty());
    }

    #[test]
    fn reads_a_vod_and_badges() {
        let data: Value =
            serde_json::from_str(r#"{"video":{"lengthSeconds":25081,"owner":{"id":"15310631"}}}"#).unwrap();
        let vod = parse_vod(&data).unwrap();
        assert_eq!(vod.length, 25081.0);
        assert_eq!(vod.channel_id, "15310631");
        let gone: Value = serde_json::from_str(r#"{"video":{"lengthSeconds":10,"owner":null}}"#).unwrap();
        assert_eq!(parse_vod(&gone), None, "a VOD whose channel is gone");
        let badges: Value = serde_json::from_str(
            r#"[{"setID":"subscriber","version":"48","title":"4-Year Subscriber","imageURL":"https://static-cdn.jtvnw.net/badges/v1/5ec1cc90-8bbc-4253-90de-2edeaa70180c/2"}]"#,
        )
        .unwrap();
        let badges = parse_badges(Some(&badges));
        assert_eq!(badges[0].set, "subscriber");
        assert_eq!(
            badges[0].images.iter().map(|i| i.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://static-cdn.jtvnw.net/badges/v1/5ec1cc90-8bbc-4253-90de-2edeaa70180c/1",
                "https://static-cdn.jtvnw.net/badges/v1/5ec1cc90-8bbc-4253-90de-2edeaa70180c/2",
                "https://static-cdn.jtvnw.net/badges/v1/5ec1cc90-8bbc-4253-90de-2edeaa70180c/3",
            ]
        );
    }

    #[test]
    fn neighbouring_pages_meet_when_they_overlap() {
        // Shared comment.
        assert!(meets(
            &page(&[("a", 10), ("b", 12)], true),
            &page(&[("b", 12), ("c", 13)], true),
            12
        ));
        // The second starts before the first ends.
        assert!(meets(
            &page(&[("a", 10), ("b", 14)], true),
            &page(&[("c", 13), ("d", 16)], true),
            15
        ));
        // A run to the end of the chat, or nothing at all, leaves nothing out.
        assert!(meets(&page(&[("a", 10)], false), &page(&[("c", 90)], true), 60));
        assert!(meets(&page(&[], true), &page(&[("c", 90)], true), 60));
        // A gap: the second starts after the first ends, sharing nothing.
        assert!(!meets(
            &page(&[("a", 10), ("b", 14)], true),
            &page(&[("c", 31)], true),
            30
        ));
        // The same second, but nothing shared: comments may be missing between them.
        assert!(!meets(
            &page(&[("a", 10), ("b", 14)], true),
            &page(&[("c", 14)], true),
            12
        ));
        // Nothing after the second's time: the first has to reach it.
        assert!(meets(&page(&[("a", 10), ("b", 31)], true), &page(&[], true), 30));
        assert!(!meets(&page(&[("a", 10), ("b", 20)], true), &page(&[], true), 30));
    }

    #[test]
    fn fills_in_where_neighbours_dont_meet_at_the_spacing_the_chat_needs() {
        let mut pages = BTreeMap::new();
        // Busy: the page at 0 reaches only 4 s, so the next 30 s are probed every 3.
        pages.insert(0, page(&[("a", 0), ("b", 4)], true));
        // Quiet: the page at 30 reaches 52 and the one at 60 starts at 58: one page between.
        pages.insert(30, page(&[("c", 29), ("d", 52)], true));
        pages.insert(60, page(&[("e", 58), ("f", 70)], true));
        let (wanted, stuck) = fill_in(&pages);
        assert_eq!(wanted, vec![3, 6, 9, 12, 15, 18, 21, 24, 27, 51]);
        assert_eq!(stuck, 0);

        // Two pages a bucket apart that still don't meet: nothing more to ask for.
        let mut dense = BTreeMap::new();
        dense.insert(3, page(&[("a", 1), ("b", 4)], true));
        dense.insert(6, page(&[("c", 5)], true));
        assert_eq!(fill_in(&dense), (Vec::new(), 1));
    }

    #[test]
    fn places_each_comment_within_its_second() {
        let comment = |id: &str, offset: u32, sent: f64| Comment {
            id: id.into(),
            offset,
            sent: Some(sent),
            name: id.into(),
            login: id.into(),
            color: None,
            badges: Vec::new(),
            parts: Vec::new(),
        };
        // The video started at 1000.25 by Twitch's count: offsets are the seconds since, rounded down.
        let comments = vec![
            comment("late", 12, 1012.75),
            comment("early", 3, 1003.25),
            comment("exact", 7, 1007.5),
        ];
        assert_eq!(origin(&comments), Some(1000.25));
        let messages = timed(comments);
        let times: Vec<(&str, f64)> = messages.iter().map(|m| (m.id.as_str(), m.time)).collect();
        assert_eq!(times, vec![("early", 3.0), ("exact", 7.25), ("late", 12.5)]);
        // Without a moment, the whole second, and no start to be had.
        let mut bare = comment("bare", 5, 0.0);
        bare.sent = None;
        assert_eq!(origin(std::slice::from_ref(&bare)), None);
        assert_eq!(timed(vec![bare])[0].time, 5.0);
    }
}

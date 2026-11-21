//! Twitch chat as it airs: Twitch's IRC, read without an account (as
//! "justinfan…", which Twitch lets anyone read with). Over TLS on port 443,
//! which proxies let through, and through the same proxy as downloads.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{BadgeRef, Message, Part};

const HOST: &str = "irc.chat.twitch.tv";
const PORT: u16 = 443;
/// Twitch says PING about every five minutes; silence for this long means the connection is gone.
const SILENCE: Duration = Duration::from_secs(6 * 60);
/// The longest wait between tries to connect again.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// What the chat said that matters for drawing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// A message, or a notice (a sub, a raid); its time is when it was sent, in seconds since 1970.
    Message(Message),
    /// A moderator deleted a message.
    Delete { id: String },
    /// A user was timed out or banned, and their recent messages taken away.
    ClearUser { login: String, at: f64 },
}

/// Reads `channel`'s chat until the task is dropped, handing each event to
/// `on_event`, connecting again (after a growing pause) whenever the
/// connection drops. `on_status` hears when it is connected, and why not.
pub async fn listen(channel: &str, mut on_event: impl FnMut(Event), mut on_status: impl FnMut(Result<(), String>)) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match session(channel, &mut on_event, &mut on_status).await {
            // Twitch asked to reconnect, or a quiet connection was closed: at once.
            Ok(()) => backoff = Duration::from_secs(1),
            Err(why) => {
                on_status(Err(why));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// One connection: joins the channel and reads it until it ends.
async fn session(
    channel: &str,
    on_event: &mut impl FnMut(Event),
    on_status: &mut impl FnMut(Result<(), String>),
) -> Result<(), String> {
    let tcp = crate::throttle::connect(HOST, PORT)
        .await
        .map_err(|err| format!("couldn't reach Twitch's chat: {err}"))?;
    let tls = native_tls::TlsConnector::new().map_err(|err| err.to_string())?;
    let stream = tokio_native_tls::TlsConnector::from(tls)
        .connect(HOST, tcp)
        .await
        .map_err(|err| format!("couldn't reach Twitch's chat securely: {err}"))?;
    let (reader, mut writer) = tokio::io::split(stream);
    let nick = format!("justinfan{}", 10_000 + now_nanos() % 89_999);
    let hello = format!(
        "CAP REQ :twitch.tv/tags twitch.tv/commands\r\nPASS SCHMOOPIIE\r\nNICK {nick}\r\nJOIN #{}\r\n",
        channel.to_ascii_lowercase()
    );
    writer
        .write_all(hello.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match tokio::time::timeout(SILENCE, lines.next_line()).await {
            Err(_) => return Ok(()),
            Ok(Err(err)) => return Err(format!("the chat connection dropped: {err}")),
            Ok(Ok(None)) => return Err("the chat connection closed".to_string()),
            Ok(Ok(Some(line))) => line,
        };
        let Some(parsed) = Line::parse(&line) else { continue };
        match parsed.command {
            "PING" => {
                let pong = format!("PONG :{}\r\n", parsed.trailing.unwrap_or("tmi.twitch.tv"));
                writer.write_all(pong.as_bytes()).await.map_err(|err| err.to_string())?;
            }
            "RECONNECT" => return Ok(()),
            "JOIN" => on_status(Ok(())),
            _ => {
                for event in events(&parsed) {
                    on_event(event);
                }
            }
        }
    }
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::from(since.subsec_nanos()))
}

/// One line of IRC with Twitch's tags: `@a=1;b=2 :prefix COMMAND #channel :text`.
#[derive(Debug)]
struct Line<'a> {
    tags: Vec<(&'a str, String)>,
    prefix: Option<&'a str>,
    command: &'a str,
    trailing: Option<&'a str>,
}

impl<'a> Line<'a> {
    fn parse(line: &'a str) -> Option<Line<'a>> {
        let mut rest = line.trim_end_matches(['\r', '\n']);
        let mut tags = Vec::new();
        if let Some(tagged) = rest.strip_prefix('@') {
            let (raw, after) = tagged.split_once(' ')?;
            tags = raw
                .split(';')
                .filter_map(|tag| tag.split_once('=').map(|(key, value)| (key, unescape(value))))
                .collect();
            rest = after;
        }
        let mut prefix = None;
        if let Some(prefixed) = rest.strip_prefix(':') {
            let (raw, after) = prefixed.split_once(' ')?;
            prefix = Some(raw);
            rest = after;
        }
        let (head, trailing) = match rest.split_once(" :") {
            Some((head, trailing)) => (head, Some(trailing)),
            None => (rest, None),
        };
        let command = head.split(' ').next().filter(|command| !command.is_empty())?;
        Some(Line {
            tags,
            prefix,
            command,
            trailing,
        })
    }

    fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    /// When Twitch's server saw it, in seconds since 1970.
    fn sent(&self) -> Option<f64> {
        self.tag("tmi-sent-ts")?
            .parse::<f64>()
            .ok()
            .map(|millis| millis / 1000.0)
    }
}

/// A tag's value, unescaped as IRCv3 says: `\s` is a space, `\:` a semicolon.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some(':') => out.push(';'),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// What a line of chat means for drawing it.
fn events(line: &Line) -> Vec<Event> {
    let Some(sent) = line.sent() else {
        return Vec::new();
    };
    match line.command {
        "PRIVMSG" => message(line, sent, line.trailing.unwrap_or_default(), false)
            .into_iter()
            .map(Event::Message)
            .collect(),
        // A sub, a gift, a raid: Twitch's line about it, then what the user wrote, if anything.
        "USERNOTICE" => {
            let mut events = Vec::new();
            if let Some(system) = line.tag("system-msg") {
                if let Some(notice) = message(line, sent, system, true) {
                    events.push(Event::Message(notice));
                }
            }
            if let Some(text) = line.trailing.filter(|text| !text.is_empty()) {
                if let Some(mut said) = message(line, sent, text, false) {
                    said.id.push_str("-said");
                    events.push(Event::Message(said));
                }
            }
            events
        }
        "CLEARMSG" => line
            .tag("target-msg-id")
            .map(|id| vec![Event::Delete { id: id.to_string() }])
            .unwrap_or_default(),
        // Without a user, the whole chat was cleared: Twitch's own replay keeps it, and so does Siphon.
        "CLEARCHAT" => line
            .trailing
            .filter(|login| !login.is_empty())
            .map(|login| {
                vec![Event::ClearUser {
                    login: login.to_ascii_lowercase(),
                    at: sent,
                }]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A message from a PRIVMSG or USERNOTICE line, `text` being what it says.
fn message(line: &Line, sent: f64, text: &str, notice: bool) -> Option<Message> {
    let login = line
        .tag("login")
        .map(str::to_string)
        .or_else(|| line.prefix?.split('!').next().map(str::to_string))
        .filter(|login| !login.is_empty() && !login.contains('.'))?;
    // `/me` arrives wrapped as a CTCP ACTION.
    let text = text
        .strip_prefix("\u{1}ACTION ")
        .map(|action| action.strip_suffix('\u{1}').unwrap_or(action))
        .unwrap_or(text);
    let parts = if notice {
        vec![Part::Text { text: text.to_string() }]
    } else {
        parts(text, line.tag("emotes").unwrap_or_default())
    };
    Some(Message {
        id: line.tag("id")?.to_string(),
        time: sent,
        name: line.tag("display-name").map_or_else(|| login.clone(), str::to_string),
        login,
        color: line.tag("color").filter(|color| color.len() == 7).map(str::to_string),
        badges: line
            .tag("badges")
            .unwrap_or_default()
            .split(',')
            .filter_map(|badge| {
                let (set, version) = badge.split_once('/')?;
                Some(BadgeRef {
                    set: set.to_string(),
                    version: version.to_string(),
                })
            })
            .collect(),
        parts,
        notice,
    })
}

/// The text cut into words and Twitch's own emotes, from the `emotes` tag:
/// `25:0-4,12-16/1902:6-10` names each emote's places in the text, counted
/// in characters.
fn parts(text: &str, emotes: &str) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let mut places: Vec<(usize, usize, &str)> = emotes
        .split('/')
        .filter_map(|emote| emote.split_once(':'))
        .flat_map(|(id, ranges)| {
            ranges.split(',').filter_map(move |range| {
                let (start, end) = range.split_once('-')?;
                Some((start.parse().ok()?, end.parse().ok()?, id))
            })
        })
        .filter(|(start, end, _)| start <= end && *end < chars.len())
        .collect();
    places.sort_by_key(|(start, ..)| *start);
    let mut parts = Vec::new();
    let mut at = 0;
    for (start, end, id) in places {
        if start < at {
            continue;
        }
        if start > at {
            parts.push(Part::Text {
                text: chars[at..start].iter().collect(),
            });
        }
        parts.push(Part::Emote {
            id: id.to_string(),
            name: chars[start..=end].iter().collect(),
        });
        at = end + 1;
    }
    if at < chars.len() {
        parts.push(Part::Text {
            text: chars[at..].iter().collect(),
        });
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as Twitch sent them on 2026-11-21 (two PRIVMSG, trimmed) and as its documentation shows the rest.
    const EMOTE_ONLY: &str = r"@badge-info=subscriber/42;badges=subscriber/42,premium/1;client-nonce=l0vei9zero;color=;display-name=leesslieramirez;emote-only=1;emotes=emotesv2_4acae216f01749f5b38bac78a82323c3:0-11;first-msg=0;flags=;id=f23f4b9b-d087-4d67-ac62-b643007e245c;mod=0;returning-chatter=0;room-id=459331509;subscriber=1;tmi-sent-ts=1795280409292;turbo=0;user-id=807773076;user-type= :leesslieramirez!leesslieramirez@leesslieramirez.tmi.twitch.tv PRIVMSG #auronplay :auronCombion";
    const PLAIN: &str = r"@badge-info=;badges=campaign-459331509-e17de7d1/1;color=#1E90FF;display-name=Carlos;emotes=25:6-10,18-22;first-msg=0;flags=;id=a112769e;mod=0;returning-chatter=0;room-id=459331509;subscriber=0;tmi-sent-ts=1795280409040;turbo=0;user-id=786414300;user-type= :carlos1597castro!carlos1597castro@carlos1597castro.tmi.twitch.tv PRIVMSG #auronplay :hello Kappa 😂 lol Kappa";
    const SUB: &str = r"@badge-info=subscriber/10;badges=subscriber/9;color=#008000;display-name=Ronni;emotes=;id=db25007f;login=ronni;msg-id=resub;system-msg=Ronni\ssubscribed\sat\sTier\s1.\sThey've\ssubscribed\sfor\s10\smonths!;tmi-sent-ts=1507246572675;user-id=1337 :tmi.twitch.tv USERNOTICE #dallas :Great stream -- keep it up!";
    const DELETED: &str = r"@login=ronni;room-id=;target-msg-id=abc-123-def;tmi-sent-ts=1642720582342 :tmi.twitch.tv CLEARMSG #dallas :HeyGuys";
    const TIMED_OUT: &str = r"@ban-duration=350;room-id=12345678;target-user-id=87654321;tmi-sent-ts=1642719320727 :tmi.twitch.tv CLEARCHAT #dallas :Ronni";
    const CLEARED: &str = r"@room-id=12345678;tmi-sent-ts=1642715695392 :tmi.twitch.tv CLEARCHAT #dallas";

    fn only_message(line: &str) -> Message {
        match events(&Line::parse(line).unwrap()).as_slice() {
            [Event::Message(message)] => message.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_a_message_with_its_badges_colour_and_emotes() {
        let message = only_message(EMOTE_ONLY);
        assert_eq!(message.id, "f23f4b9b-d087-4d67-ac62-b643007e245c");
        assert_eq!(
            (message.name.as_str(), message.login.as_str()),
            ("leesslieramirez", "leesslieramirez")
        );
        assert!((message.time - 1_795_280_409.292).abs() < 1e-6);
        assert_eq!(message.color, None);
        assert_eq!(message.badges.len(), 2);
        assert_eq!(message.badges[0].set, "subscriber");
        assert_eq!(
            message.parts,
            vec![Part::Emote {
                id: "emotesv2_4acae216f01749f5b38bac78a82323c3".into(),
                name: "auronCombion".into()
            }]
        );

        let plain = only_message(PLAIN);
        assert_eq!(plain.color.as_deref(), Some("#1E90FF"));
        let kappa = || Part::Emote {
            id: "25".into(),
            name: "Kappa".into(),
        };
        // Counted in characters: the emoji is one.
        assert_eq!(
            plain.parts,
            vec![
                Part::Text { text: "hello ".into() },
                kappa(),
                Part::Text {
                    text: " 😂 lol ".into()
                },
                kappa(),
            ]
        );
    }

    #[test]
    fn a_sub_is_twitchs_line_then_what_the_user_wrote() {
        let events = events(&Line::parse(SUB).unwrap());
        let [Event::Message(notice), Event::Message(said)] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert!(notice.notice && !said.notice);
        assert_eq!(
            notice.parts,
            vec![Part::Text {
                text: "Ronni subscribed at Tier 1. They've subscribed for 10 months!".into()
            }]
        );
        assert_eq!(
            said.parts,
            vec![Part::Text {
                text: "Great stream -- keep it up!".into()
            }]
        );
        assert_ne!(notice.id, said.id);
        assert_eq!(said.login, "ronni");
    }

    #[test]
    fn reads_deletions_and_timeouts_but_keeps_a_cleared_chat() {
        assert_eq!(
            events(&Line::parse(DELETED).unwrap()),
            vec![Event::Delete {
                id: "abc-123-def".into()
            }]
        );
        assert_eq!(
            events(&Line::parse(TIMED_OUT).unwrap()),
            vec![Event::ClearUser {
                login: "ronni".into(),
                at: 1_642_719_320.727
            }]
        );
        assert!(events(&Line::parse(CLEARED).unwrap()).is_empty());
    }

    #[test]
    fn reads_pings_actions_and_escapes() {
        let ping = Line::parse("PING :tmi.twitch.tv").unwrap();
        assert_eq!((ping.command, ping.trailing), ("PING", Some("tmi.twitch.tv")));
        let action = only_message(
            "@display-name=A;id=1;tmi-sent-ts=1000 :a!a@a.tmi.twitch.tv PRIVMSG #c :\u{1}ACTION waves\u{1}",
        );
        assert_eq!(action.parts, vec![Part::Text { text: "waves".into() }]);
        assert_eq!(unescape(r"a\sb\:c\\d"), r"a b;c\d");
        // A line without Twitch's time isn't drawn: it can't be placed.
        assert!(events(&Line::parse(":a!a@a PRIVMSG #c :hi").unwrap()).is_empty());
        // Emote places outside the text are ignored rather than trusted.
        assert_eq!(parts("hi", "25:0-9"), vec![Part::Text { text: "hi".into() }]);
    }
}

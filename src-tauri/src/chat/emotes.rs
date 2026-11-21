//! The third-party emotes a channel's chat shows: 7TV's, BTTV's and FFZ's,
//! the channel's own sets and everyone's. Viewers with the browser
//! extensions see these words as images; so does the file.
//!
//! Where two share a name, the first of these wins: the channel's 7TV, BTTV
//! and FFZ emotes, then everyone's 7TV, BTTV and FFZ emotes. Channel sets
//! before global ones is what every chat client does; among services, 7TV
//! first, as its extension (the most used) does. FFZ's sets come through
//! BTTV's copy of them, which answers in milliseconds where FFZ's own API
//! timed out (2026-11-21).

use serde_json::Value;

use super::{Emote, Image, Provider};

const SEVEN_TV: &str = "https://7tv.io/v3";
const BTTV: &str = "https://api.betterttv.net/3/cached";

/// BTTV's emotes that are drawn over the one before them. BTTV's API
/// doesn't say which; this is the list chat clients keep (Chatterino's).
const BTTV_OVERLAYS: &[&str] = &[
    "SoSnowy",
    "IceCold",
    "SantaHat",
    "TopHat",
    "ReinDeer",
    "CandyCane",
    "cvMask",
    "cvHazmat",
];

/// 7TV's flag on an emote in a set: drawn over the one before it.
const SEVEN_TV_SET_OVERLAY: u64 = 1;
/// 7TV's flag on the emote itself: made to be drawn over another.
const SEVEN_TV_EMOTE_OVERLAY: u64 = 1 << 8;

/// A channel's and everyone's third-party emotes, merged (see the module's
/// notes), and which of the six lists couldn't be had, for the log.
pub async fn fetch(http: &reqwest::Client, channel_id: &str) -> (Vec<Emote>, Vec<&'static str>) {
    let get = |url: String| async move {
        let response = http.get(url).send().await.ok()?;
        // 7TV answers 404 for a streamer without a 7TV account: no emotes, not a failure.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Some(Value::Null);
        }
        response.error_for_status().ok()?.json::<Value>().await.ok()
    };
    let (seven_channel, bttv_channel, ffz_channel, seven_global, bttv_global, ffz_global) = tokio::join!(
        get(format!("{SEVEN_TV}/users/twitch/{channel_id}")),
        get(format!("{BTTV}/users/twitch/{channel_id}")),
        get(format!("{BTTV}/frankerfacez/users/twitch/{channel_id}")),
        get(format!("{SEVEN_TV}/emote-sets/global")),
        get(format!("{BTTV}/emotes/global")),
        get(format!("{BTTV}/frankerfacez/emotes/global")),
    );
    let mut missing = Vec::new();
    let mut lists = Vec::new();
    let mut take = |answer: Option<Value>, name: &'static str, parse: fn(&Value) -> Vec<Emote>| match answer {
        Some(value) => lists.push(parse(&value)),
        None => missing.push(name),
    };
    take(seven_channel, "7TV channel", |v| {
        seven_tv(v.pointer("/emote_set/emotes"))
    });
    take(bttv_channel, "BTTV channel", bttv_channel_emotes);
    take(ffz_channel, "FFZ channel", |v| ffz(Some(v)));
    take(seven_global, "7TV global", |v| seven_tv(v.get("emotes")));
    take(bttv_global, "BTTV global", |v| bttv(Some(v)));
    take(ffz_global, "FFZ global", |v| ffz(Some(v)));
    (merge(lists), missing)
}

/// The lists in order of priority, as one list in which each name appears
/// once: the first list that has it wins.
pub fn merge(lists: Vec<Vec<Emote>>) -> Vec<Emote> {
    let mut seen = std::collections::HashSet::new();
    lists
        .into_iter()
        .flatten()
        .filter(|emote| !emote.name.is_empty() && seen.insert(emote.name.clone()))
        .collect()
}

/// 7TV's emotes in a set (`emote_set.emotes` of a user, `emotes` of the
/// global set). The name is the one the set gives it, which can differ from
/// the emote's own.
fn seven_tv(list: Option<&Value>) -> Vec<Emote> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|emote| {
            let name = emote.get("name")?.as_str()?;
            let data = emote.get("data")?;
            let host = data.get("host")?;
            let base = host.get("url")?.as_str()?;
            let base = if base.starts_with("//") {
                format!("https:{base}")
            } else {
                base.to_string()
            };
            let mut images: Vec<Image> = host
                .get("files")?
                .as_array()?
                .iter()
                .filter(|file| file.get("format").and_then(Value::as_str) == Some("WEBP"))
                .filter_map(|file| {
                    Some(Image {
                        height: file.get("height")?.as_u64()? as u32,
                        url: format!("{base}/{}", file.get("name")?.as_str()?),
                    })
                })
                .collect();
            images.sort_by_key(|image| image.height);
            let flags = |value: &Value| value.get("flags").and_then(Value::as_u64).unwrap_or(0);
            Some(Emote {
                name: name.to_string(),
                provider: Provider::SevenTv,
                id: emote.get("id")?.as_str()?.to_string(),
                images,
                overlay: flags(emote) & SEVEN_TV_SET_OVERLAY != 0 || flags(data) & SEVEN_TV_EMOTE_OVERLAY != 0,
            })
            .filter(|emote| !emote.images.is_empty())
        })
        .collect()
}

/// A channel on BTTV: its own emotes, then those it shares from others.
fn bttv_channel_emotes(value: &Value) -> Vec<Emote> {
    let mut emotes = bttv(value.get("channelEmotes"));
    emotes.extend(bttv(value.get("sharedEmotes")));
    emotes
}

/// BTTV's emotes. Its "modifiers" (`w!`, `h!`…) change the next emote in
/// BTTV's extension; Siphon leaves them out rather than show them as words.
fn bttv(list: Option<&Value>) -> Vec<Emote> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter(|emote| emote.get("modifier").and_then(Value::as_bool) != Some(true))
        .filter_map(|emote| {
            let id = emote.get("id")?.as_str()?;
            let name = emote.get("code")?.as_str()?;
            Some(Emote {
                name: name.to_string(),
                provider: Provider::Bttv,
                id: id.to_string(),
                images: [(28, "1x"), (56, "2x"), (112, "3x")]
                    .into_iter()
                    .map(|(height, size)| Image {
                        height,
                        url: format!("https://cdn.betterttv.net/emote/{id}/{size}.webp"),
                    })
                    .collect(),
                overlay: BTTV_OVERLAYS.contains(&name),
            })
        })
        .collect()
}

/// FFZ's emotes, as BTTV's copy lists them. FFZ's "modifier" emotes are
/// drawn over the emote before them.
fn ffz(list: Option<&Value>) -> Vec<Emote> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|emote| {
            let id = match emote.get("id")? {
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.clone(),
                _ => return None,
            };
            let images = emote.get("images")?;
            let images: Vec<Image> = [(28, "1x"), (56, "2x"), (112, "4x")]
                .into_iter()
                .filter_map(|(height, size)| {
                    Some(Image {
                        height,
                        url: images.get(size)?.as_str()?.to_string(),
                    })
                })
                .collect();
            Some(Emote {
                name: emote.get("code")?.as_str()?.to_string(),
                provider: Provider::Ffz,
                id,
                images,
                overlay: emote.get("modifier").and_then(Value::as_bool) == Some(true),
            })
            .filter(|emote| !emote.images.is_empty())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn reads_7tv_sets_with_their_names_sizes_and_overlays() {
        // A user's set as 7TV's v3 API answered on 2026-11-21 (trimmed).
        let user = parse(
            r#"{"id":"1","emote_set":{"id":"01HT044CVG0008NBPM0BA6DFYN","name":"Add Emotes Here","emotes":[
                {"id":"01FHMGJRDR000FFDX954T14FX0","name":"Bob","flags":0,"data":{"id":"01FHMGJRDR000FFDX954T14FX0","name":"ReallyNow","flags":0,"animated":false,
                 "host":{"url":"//cdn.7tv.app/emote/01FHMGJRDR000FFDX954T14FX0","files":[
                   {"name":"2x.webp","width":64,"height":64,"format":"WEBP"},{"name":"1x.avif","width":32,"height":32,"format":"AVIF"},{"name":"1x.webp","width":32,"height":32,"format":"WEBP"}]}}},
                {"id":"02","name":"MILK","flags":0,"data":{"flags":256,"host":{"url":"//cdn.7tv.app/emote/02","files":[{"name":"1x.webp","height":32,"format":"WEBP"}]}}},
                {"id":"03","name":"RainTime","flags":1,"data":{"flags":0,"host":{"url":"//cdn.7tv.app/emote/03","files":[{"name":"1x.webp","height":32,"format":"WEBP"}]}}},
                {"id":"04","name":"OnlyAvif","flags":0,"data":{"flags":0,"host":{"url":"//cdn.7tv.app/emote/04","files":[{"name":"1x.avif","height":32,"format":"AVIF"}]}}}
            ]}}"#,
        );
        let emotes = seven_tv(user.pointer("/emote_set/emotes"));
        assert_eq!(emotes.len(), 3, "an emote with no WebP image is left out");
        assert_eq!(emotes[0].name, "Bob", "the set's name for it, not the emote's own");
        assert_eq!(
            emotes[0].images,
            vec![
                Image {
                    height: 32,
                    url: "https://cdn.7tv.app/emote/01FHMGJRDR000FFDX954T14FX0/1x.webp".into()
                },
                Image {
                    height: 64,
                    url: "https://cdn.7tv.app/emote/01FHMGJRDR000FFDX954T14FX0/2x.webp".into()
                },
            ]
        );
        assert_eq!(
            emotes.iter().map(|e| e.overlay).collect::<Vec<_>>(),
            vec![false, true, true]
        );
        // A streamer without 7TV: nothing.
        assert!(seven_tv(Value::Null.pointer("/emote_set/emotes")).is_empty());
    }

    #[test]
    fn reads_bttv_and_ffz_leaving_out_bttvs_modifiers() {
        let bttv_user = parse(
            r#"{"id":"x","channelEmotes":[{"id":"5695bb786bc784fb79bada3a","code":"GreekG","imageType":"png","animated":false}],
                "sharedEmotes":[{"id":"5f08965672ac200523c66ddb","code":"Hammies","imageType":"gif","animated":true}]}"#,
        );
        let emotes = bttv_channel_emotes(&bttv_user);
        assert_eq!(
            emotes.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["GreekG", "Hammies"]
        );
        assert_eq!(
            emotes[1].images[1].url,
            "https://cdn.betterttv.net/emote/5f08965672ac200523c66ddb/2x.webp"
        );
        let global = parse(
            r#"[{"id":"1","code":":tf:","imageType":"png","animated":false,"modifier":false},
                {"id":"2","code":"w!","imageType":"png","animated":false,"modifier":true},
                {"id":"3","code":"IceCold","imageType":"png","animated":false,"modifier":false}]"#,
        );
        let global = bttv(Some(&global));
        assert_eq!(
            global.iter().map(|e| (e.name.as_str(), e.overlay)).collect::<Vec<_>>(),
            vec![(":tf:", false), ("IceCold", true)]
        );

        let ffz_list = parse(
            r#"[{"id":523340,"user":{"id":313221,"name":"timmytoina"},"code":"3Lass","images":{"1x":"https://cdn.betterttv.net/frankerfacez_emote/523340/1","2x":"https://cdn.betterttv.net/frankerfacez_emote/523340/2","4x":null},"imageType":"png","animated":false,"modifier":false},
                {"id":7,"code":"ffzHat","images":{"1x":"https://cdn.betterttv.net/frankerfacez_emote/7/1"},"modifier":true}]"#,
        );
        let ffz_emotes = ffz(Some(&ffz_list));
        assert_eq!(ffz_emotes[0].id, "523340");
        assert_eq!(ffz_emotes[0].images.len(), 2, "sizes FFZ doesn't have are left out");
        assert!(ffz_emotes[1].overlay);
    }

    #[test]
    fn the_first_list_to_name_an_emote_wins() {
        let emote = |name: &str, provider: Provider| Emote {
            name: name.into(),
            provider,
            id: name.into(),
            images: Vec::new(),
            overlay: false,
        };
        let merged = merge(vec![
            vec![emote("KEKW", Provider::SevenTv)],
            vec![emote("KEKW", Provider::Bttv), emote("Hammies", Provider::Bttv)],
            vec![emote("Hammies", Provider::Ffz), emote("", Provider::Ffz)],
        ]);
        assert_eq!(
            merged.iter().map(|e| (e.name.as_str(), e.provider)).collect::<Vec<_>>(),
            vec![("KEKW", Provider::SevenTv), ("Hammies", Provider::Bttv)]
        );
    }
}

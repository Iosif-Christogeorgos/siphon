//! The subtitles a video offers, as `yt-dlp -J` lists them, and the names of
//! their languages.
//!
//! yt-dlp lists them in two places, by language: `subtitles` (written by
//! people: the creator, or a site's own team) and `automatic_captions`
//! (made by machine). YouTube's machine-made ones are of two kinds: speech
//! recognition in the language spoken, its key ending in `-orig` ("en-orig",
//! named "English (Original)"), and machine translations of that into about
//! 160 languages ("el" for Greek, fetched with `tlang=el`). It also lists the
//! people-written tracks a second time among the machine-made ones, as HLS
//! copies. What one track holds is the same everywhere: a list of files, one
//! per format the site offers it in.
//!
//! Siphon reads them into one list, each track marked with its kind, leaving
//! out what isn't subtitles (a live chat replay, Bilibili's danmaku), tracks
//! only in formats nothing here can read (YouTube's json3 and srv), and
//! YouTube's machine translations: it refused every one asked for in testing
//! (2026-11-20, four of four, on two videos and spaced 20 s apart: HTTP 429),
//! so offering them would offer what almost never comes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who made a subtitle track. Ordered as Siphon prefers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleKind {
    /// Written by people: the creator, or the site.
    Uploaded,
    /// Made by speech recognition, in the language spoken. Often wrong.
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubtitleTrack {
    /// yt-dlp's name for the track, which is what it is asked for by:
    /// "en", "de-DE", "en-orig", "pt-br".
    pub key: String,
    /// Its language as a lowercase tag: "en", "de-de", "pt-br" (see [`language_tag`]).
    pub lang: String,
    /// The site's name for it, if it gives one: "English", "German (Germany)".
    pub name: Option<String>,
    pub kind: SubtitleKind,
    /// The file types it comes in, as yt-dlp names them, only those Siphon can
    /// use: "srt", "vtt", "ass", "ttml".
    #[serde(skip)]
    pub exts: Vec<String>,
}

impl SubtitleTrack {
    pub fn offers(&self, ext: &str) -> bool {
        self.exts.iter().any(|offered| offered == ext)
    }
}

/// Formats that can be read here: by ffmpeg (SubRip, WebVTT, SubStation), or
/// turned into SubRip by yt-dlp itself (TTML and its older name, DFXP).
const READABLE: &[&str] = &["srt", "vtt", "ass", "ssa", "ttml", "dfxp"];

/// What yt-dlp lists with the subtitles but isn't subtitles.
const NOT_SUBTITLES: &[&str] = &["live_chat", "rechat", "danmaku", "comments"];

/// The suffix of YouTube's speech-recognition track.
const ORIGINAL: &str = "-orig";

/// Reads `subtitles` and `automatic_captions` from yt-dlp's result. People's
/// tracks first, then speech recognition, each kind by key.
pub fn read(uploaded: Option<&Value>, automatic: Option<&Value>) -> Vec<SubtitleTrack> {
    let mut tracks: Vec<SubtitleTrack> = entries(uploaded)
        .filter_map(|(key, files)| track(key, files, SubtitleKind::Uploaded))
        .collect();

    let automatic: Vec<(&String, &Value)> = entries(automatic).collect();
    // YouTube's shape: one track from speech, the rest translated from it.
    let spoken: Vec<String> = automatic
        .iter()
        .filter(|(key, _)| key.ends_with(ORIGINAL))
        .map(|(key, _)| language_tag(key))
        .collect();
    for (key, files) in automatic {
        let original = key.ends_with(ORIGINAL);
        // A people's track listed again (YouTube's HLS copies), or the speech
        // track listed again under its plain language ("en" beside "en-orig").
        let repeated =
            tracks.iter().any(|track| track.key == *key) || (!original && spoken.contains(&language_tag(key)));
        // Beside a speech track, every other one is a machine translation of it (see above).
        let translated = !original && !spoken.is_empty();
        if repeated || translated {
            continue;
        }
        tracks.extend(track(key, files, SubtitleKind::Automatic));
    }
    tracks.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    tracks
}

/// The language → files entries of one of yt-dlp's subtitle lists.
fn entries(list: Option<&Value>) -> impl Iterator<Item = (&String, &Value)> {
    list.and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, _)| !NOT_SUBTITLES.contains(&key.as_str()) && !key.trim().is_empty())
}

fn track(key: &str, files: &Value, kind: SubtitleKind) -> Option<SubtitleTrack> {
    let files = files.as_array()?;
    let mut exts: Vec<String> = Vec::new();
    for file in files {
        let Some(ext) = file.get("ext").and_then(Value::as_str).map(str::to_ascii_lowercase) else {
            continue;
        };
        if READABLE.contains(&ext.as_str()) && !exts.contains(&ext) {
            exts.push(ext);
        }
    }
    if exts.is_empty() {
        return None;
    }
    let name = files
        .iter()
        .filter_map(|file| file.get("name").and_then(Value::as_str))
        .map(|name| name.trim().trim_end_matches(" (Original)").trim().to_string())
        .find(|name| !name.is_empty());
    Some(SubtitleTrack {
        key: key.to_string(),
        lang: language_tag(key),
        name,
        kind,
        exts,
    })
}

/// A track's language as a lowercase tag, the same however the site writes
/// it: "de-DE" → "de-de", "pt_BR" → "pt-br", "en-orig" → "en", "eng" → "en".
pub fn language_tag(key: &str) -> String {
    let key = key.trim();
    let key = key.strip_suffix(ORIGINAL).unwrap_or(key);
    let tag = key.replace('_', "-").to_ascii_lowercase();
    let (primary, rest) = match tag.split_once('-') {
        Some((primary, rest)) => (primary, Some(rest)),
        None => (tag.as_str(), None),
    };
    let primary = LANGUAGES
        .iter()
        .find(|(_, bibliographic, terminology, _)| primary == *bibliographic || primary == *terminology)
        .map_or(primary, |(two, ..)| two);
    match rest {
        Some(rest) => format!("{primary}-{rest}"),
        None => primary.to_string(),
    }
}

/// The part of a tag that names the language alone: "pt-br" → "pt".
pub fn primary(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// The three-letter code (ISO 639-2) for a language tag, which is what MP4
/// and Matroska files name a track's language by: MP4 drops a two-letter one
/// (tested 2026-11-20: "el" came out with no language, "ell" as Greek).
/// Matroska takes the bibliographic form ("gre"), MP4 the terminology form
/// ("ell"); they differ for some twenty languages. None for a language not
/// in the table: the track is then named but not tagged.
pub fn iso639_2(tag: &str, bibliographic: bool) -> Option<&str> {
    let primary = primary(tag);
    if primary.len() == 3 && primary.chars().all(|c| c.is_ascii_lowercase()) {
        return Some(primary);
    }
    LANGUAGES
        .iter()
        .find(|(two, ..)| *two == primary)
        .map(|(_, b, t, _)| if bibliographic { *b } else { *t })
}

/// The language's name in English, for messages ("No Greek subtitles"):
/// "pt-br" → "Portuguese". None for a language not in the table.
pub fn language_name(tag: &str) -> Option<&'static str> {
    let primary = primary(tag);
    LANGUAGES
        .iter()
        .find(|(two, b, t, _)| [*two, *b, *t].contains(&primary))
        .map(|(.., name)| *name)
}

/// The tag of the language called `name` in English, whatever its capitals:
/// "greek" → "el". None for a name not in the table.
pub fn language_named(name: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(.., english)| english.eq_ignore_ascii_case(name))
        .map(|(two, ..)| *two)
}

/// ISO 639-1, ISO 639-2 bibliographic and terminology codes, and the name in
/// English: the languages YouTube translates into, and others common on video sites.
const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    ("aa", "aar", "aar", "Afar"),
    ("ab", "abk", "abk", "Abkhazian"),
    ("af", "afr", "afr", "Afrikaans"),
    ("ak", "aka", "aka", "Akan"),
    ("am", "amh", "amh", "Amharic"),
    ("ar", "ara", "ara", "Arabic"),
    ("as", "asm", "asm", "Assamese"),
    ("ay", "aym", "aym", "Aymara"),
    ("az", "aze", "aze", "Azerbaijani"),
    ("ba", "bak", "bak", "Bashkir"),
    ("be", "bel", "bel", "Belarusian"),
    ("bg", "bul", "bul", "Bulgarian"),
    ("bn", "ben", "ben", "Bangla"),
    ("bo", "tib", "bod", "Tibetan"),
    ("br", "bre", "bre", "Breton"),
    ("bs", "bos", "bos", "Bosnian"),
    ("ca", "cat", "cat", "Catalan"),
    ("co", "cos", "cos", "Corsican"),
    ("cs", "cze", "ces", "Czech"),
    ("cy", "wel", "cym", "Welsh"),
    ("da", "dan", "dan", "Danish"),
    ("de", "ger", "deu", "German"),
    ("dv", "div", "div", "Divehi"),
    ("dz", "dzo", "dzo", "Dzongkha"),
    ("ee", "ewe", "ewe", "Ewe"),
    ("el", "gre", "ell", "Greek"),
    ("en", "eng", "eng", "English"),
    ("eo", "epo", "epo", "Esperanto"),
    ("es", "spa", "spa", "Spanish"),
    ("et", "est", "est", "Estonian"),
    ("eu", "baq", "eus", "Basque"),
    ("fa", "per", "fas", "Persian"),
    ("ff", "ful", "ful", "Fula"),
    ("fi", "fin", "fin", "Finnish"),
    ("fj", "fij", "fij", "Fijian"),
    ("fo", "fao", "fao", "Faroese"),
    ("fr", "fre", "fra", "French"),
    ("fy", "fry", "fry", "Western Frisian"),
    ("ga", "gle", "gle", "Irish"),
    ("gd", "gla", "gla", "Scottish Gaelic"),
    ("gl", "glg", "glg", "Galician"),
    ("gn", "grn", "grn", "Guarani"),
    ("gu", "guj", "guj", "Gujarati"),
    ("gv", "glv", "glv", "Manx"),
    ("ha", "hau", "hau", "Hausa"),
    ("he", "heb", "heb", "Hebrew"),
    ("hi", "hin", "hin", "Hindi"),
    ("hr", "hrv", "hrv", "Croatian"),
    ("ht", "hat", "hat", "Haitian Creole"),
    ("hu", "hun", "hun", "Hungarian"),
    ("hy", "arm", "hye", "Armenian"),
    ("id", "ind", "ind", "Indonesian"),
    ("ig", "ibo", "ibo", "Igbo"),
    ("is", "ice", "isl", "Icelandic"),
    ("it", "ita", "ita", "Italian"),
    ("iu", "iku", "iku", "Inuktitut"),
    ("ja", "jpn", "jpn", "Japanese"),
    ("jv", "jav", "jav", "Javanese"),
    ("ka", "geo", "kat", "Georgian"),
    ("kk", "kaz", "kaz", "Kazakh"),
    ("kl", "kal", "kal", "Kalaallisut"),
    ("km", "khm", "khm", "Khmer"),
    ("kn", "kan", "kan", "Kannada"),
    ("ko", "kor", "kor", "Korean"),
    ("ku", "kur", "kur", "Kurdish"),
    ("ky", "kir", "kir", "Kyrgyz"),
    ("la", "lat", "lat", "Latin"),
    ("lb", "ltz", "ltz", "Luxembourgish"),
    ("lg", "lug", "lug", "Ganda"),
    ("ln", "lin", "lin", "Lingala"),
    ("lo", "lao", "lao", "Lao"),
    ("lt", "lit", "lit", "Lithuanian"),
    ("lv", "lav", "lav", "Latvian"),
    ("mg", "mlg", "mlg", "Malagasy"),
    ("mi", "mao", "mri", "Māori"),
    ("mk", "mac", "mkd", "Macedonian"),
    ("ml", "mal", "mal", "Malayalam"),
    ("mn", "mon", "mon", "Mongolian"),
    ("mr", "mar", "mar", "Marathi"),
    ("ms", "may", "msa", "Malay"),
    ("mt", "mlt", "mlt", "Maltese"),
    ("my", "bur", "mya", "Burmese"),
    ("nb", "nob", "nob", "Norwegian Bokmål"),
    ("ne", "nep", "nep", "Nepali"),
    ("nl", "dut", "nld", "Dutch"),
    ("nn", "nno", "nno", "Norwegian Nynorsk"),
    ("no", "nor", "nor", "Norwegian"),
    ("ny", "nya", "nya", "Nyanja"),
    ("oc", "oci", "oci", "Occitan"),
    ("om", "orm", "orm", "Oromo"),
    ("or", "ori", "ori", "Odia"),
    ("os", "oss", "oss", "Ossetic"),
    ("pa", "pan", "pan", "Punjabi"),
    ("pl", "pol", "pol", "Polish"),
    ("ps", "pus", "pus", "Pashto"),
    ("pt", "por", "por", "Portuguese"),
    ("qu", "que", "que", "Quechua"),
    ("rm", "roh", "roh", "Romansh"),
    ("ro", "rum", "ron", "Romanian"),
    ("ru", "rus", "rus", "Russian"),
    ("rw", "kin", "kin", "Kinyarwanda"),
    ("sa", "san", "san", "Sanskrit"),
    ("sd", "snd", "snd", "Sindhi"),
    ("sg", "sag", "sag", "Sango"),
    ("si", "sin", "sin", "Sinhala"),
    ("sk", "slo", "slk", "Slovak"),
    ("sl", "slv", "slv", "Slovenian"),
    ("sm", "smo", "smo", "Samoan"),
    ("sn", "sna", "sna", "Shona"),
    ("so", "som", "som", "Somali"),
    ("sq", "alb", "sqi", "Albanian"),
    ("sr", "srp", "srp", "Serbian"),
    ("ss", "ssw", "ssw", "Swati"),
    ("st", "sot", "sot", "Southern Sotho"),
    ("su", "sun", "sun", "Sundanese"),
    ("sv", "swe", "swe", "Swedish"),
    ("sw", "swa", "swa", "Swahili"),
    ("ta", "tam", "tam", "Tamil"),
    ("te", "tel", "tel", "Telugu"),
    ("tg", "tgk", "tgk", "Tajik"),
    ("th", "tha", "tha", "Thai"),
    ("ti", "tir", "tir", "Tigrinya"),
    ("tk", "tuk", "tuk", "Turkmen"),
    ("tl", "tgl", "tgl", "Tagalog"),
    ("tn", "tsn", "tsn", "Tswana"),
    ("to", "ton", "ton", "Tongan"),
    ("tr", "tur", "tur", "Turkish"),
    ("ts", "tso", "tso", "Tsonga"),
    ("tt", "tat", "tat", "Tatar"),
    ("ug", "uig", "uig", "Uyghur"),
    ("uk", "ukr", "ukr", "Ukrainian"),
    ("ur", "urd", "urd", "Urdu"),
    ("uz", "uzb", "uzb", "Uzbek"),
    ("ve", "ven", "ven", "Venda"),
    ("vi", "vie", "vie", "Vietnamese"),
    ("wo", "wol", "wol", "Wolof"),
    ("xh", "xho", "xho", "Xhosa"),
    ("yi", "yid", "yid", "Yiddish"),
    ("yo", "yor", "yor", "Yoruba"),
    ("zh", "chi", "zho", "Chinese"),
    ("zu", "zul", "zul", "Zulu"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(name: &str, exts: &[&str]) -> Value {
        Value::Array(
            exts.iter()
                .map(|ext| json!({"ext": ext, "name": name, "url": "https://x"}))
                .collect(),
        )
    }

    /// The shape yt-dlp 2026.10.26 gave for a YouTube music video (2026-11-20),
    /// cut down: people's tracks, speech recognition in English, its
    /// translations, and HLS copies of the people's tracks among them.
    #[test]
    fn reads_youtube_people_and_speech_and_leaves_translations_out() {
        let all = &["json3", "srv1", "srv2", "srv3", "ttml", "srt", "vtt"];
        let uploaded = json!({
            "en": files("English", all),
            "de-DE": files("German (Germany)", all),
            "pt-BR": files("Portuguese (Brazil)", all),
        });
        let hls_copy = json!([{"ext": "vtt", "url": "https://x/index.m3u8", "protocol": "m3u8_native"}]);
        let automatic = json!({
            "en": [{"ext": "vtt", "protocol": "m3u8_native"}, {"ext": "srt", "name": "English"}],
            "de-DE": hls_copy,
            "en-orig": files("English (Original)", all),
            "el": files("Greek", all),
            "ja": files("Japanese", all),
        });
        let tracks = read(Some(&uploaded), Some(&automatic));
        let listed: Vec<(&str, &str, Option<&str>, SubtitleKind)> = tracks
            .iter()
            .map(|track| {
                (
                    track.key.as_str(),
                    track.lang.as_str(),
                    track.name.as_deref(),
                    track.kind,
                )
            })
            .collect();
        use SubtitleKind::*;
        assert_eq!(
            listed,
            [
                ("de-DE", "de-de", Some("German (Germany)"), Uploaded),
                ("en", "en", Some("English"), Uploaded),
                ("pt-BR", "pt-br", Some("Portuguese (Brazil)"), Uploaded),
                ("en-orig", "en", Some("English"), Automatic),
            ]
        );
        // Only formats that can be read here are kept.
        assert_eq!(tracks[1].exts, ["ttml", "srt", "vtt"]);
    }

    #[test]
    fn a_site_without_youtubes_shape_has_only_machine_made_tracks() {
        // TED (2026-11-20): people's tracks in WebVTT only, without names.
        let ted = json!({"pt-br": [{"ext": "vtt", "url": "https://x"}], "el": [{"ext": "vtt"}]});
        let tracks = read(Some(&ted), Some(&json!({})));
        assert_eq!(tracks.len(), 2);
        assert_eq!((tracks[1].lang.as_str(), tracks[1].name.as_deref()), ("pt-br", None));
        // Machine-made tracks on a site that doesn't translate them are its speech recognition.
        let auto = json!({"en": [{"ext": "vtt"}], "fr": [{"ext": "vtt"}]});
        assert!(read(None, Some(&auto))
            .iter()
            .all(|track| track.kind == SubtitleKind::Automatic));
    }

    #[test]
    fn leaves_out_what_isnt_subtitles_or_cant_be_read() {
        let uploaded = json!({
            "live_chat": [{"ext": "json", "protocol": "youtube_live_chat"}],
            "danmaku": [{"ext": "xml"}],
            "en": [{"ext": "json3"}, {"ext": "srv3"}],
            "fr": [{"ext": "SRT"}],
            "": [{"ext": "srt"}],
        });
        let tracks = read(Some(&uploaded), None);
        assert_eq!(tracks.len(), 1);
        assert_eq!(
            (tracks[0].key.as_str(), tracks[0].exts.as_slice()),
            ("fr", &["srt".to_string()][..])
        );
        // Nothing listed, or not a list at all.
        assert!(read(None, None).is_empty());
        assert!(read(Some(&json!("nonsense")), Some(&json!(null))).is_empty());
    }

    #[test]
    fn writes_every_language_one_way() {
        assert_eq!(language_tag("de-DE"), "de-de");
        assert_eq!(language_tag("pt_BR"), "pt-br");
        assert_eq!(language_tag("en-orig"), "en");
        assert_eq!(language_tag("eng"), "en");
        assert_eq!(language_tag("ger"), "de");
        assert_eq!(language_tag("zho-Hant"), "zh-hant");
        assert_eq!(language_tag("es-419"), "es-419");
        // A three-letter language with no two-letter code stays as it is.
        assert_eq!(language_tag("fil"), "fil");
        assert_eq!(primary("pt-br"), "pt");
    }

    #[test]
    fn names_a_language_in_three_letters_for_the_file() {
        assert_eq!(iso639_2("el", false), Some("ell"));
        assert_eq!(iso639_2("el", true), Some("gre"));
        assert_eq!(iso639_2("de-de", false), Some("deu"));
        assert_eq!(iso639_2("en", true), Some("eng"));
        assert_eq!(iso639_2("fil", true), Some("fil"));
        assert_eq!(iso639_2("xx", true), None);
        assert_eq!(language_name("pt-br"), Some("Portuguese"));
        assert_eq!(language_name("ger"), Some("German"));
        assert_eq!(language_name("xx"), None);
    }
}

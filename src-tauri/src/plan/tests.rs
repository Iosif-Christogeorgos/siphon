use super::registry::{MusicFormat, TargetId, TierId};
use super::*;
use crate::ffmpeg::{self, Pass};
use crate::ytdlp::args;
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};
use std::path::Path;

fn video(id: &str, vcodec: &str, height: u32, fps: f64, kbps: f64) -> Format {
    Format {
        id: id.into(),
        ext: if vcodec.starts_with("avc") || vcodec.starts_with("av01") {
            "mp4".into()
        } else {
            "webm".into()
        },
        kind: FormatKind::Video,
        vcodec: Some(vcodec.into()),
        acodec: None,
        width: Some(height * 16 / 9),
        height: Some(height),
        fps: Some(fps),
        tbr: Some(kbps),
        vbr: Some(kbps),
        abr: None,
        asr: None,
        audio_channels: None,
        // Ten minutes at the stated bitrate.
        filesize: Some((kbps * 1000.0 / 8.0 * 600.0) as u64),
        filesize_estimated: false,
        dynamic_range: Some("SDR".into()),
        language: None,
        language_preference: None,
        segmented: false,
        stream_url: None,
        http_chunk_size: None,
        http_headers: Vec::new(),
        note: None,
        video_codec: None,
        audio_codec: None,
    }
}

fn audio(id: &str, acodec: &str, kbps: f64) -> Format {
    Format {
        kind: FormatKind::Audio,
        ext: if acodec == "opus" { "webm".into() } else { "m4a".into() },
        vcodec: None,
        acodec: Some(acodec.into()),
        width: None,
        height: None,
        fps: None,
        vbr: None,
        abr: Some(kbps),
        dynamic_range: None,
        ..video(id, "none", 0, 0.0, kbps)
    }
}

fn muxed(id: &str, height: u32, kbps: f64) -> Format {
    Format {
        kind: FormatKind::Muxed,
        acodec: Some("mp4a.40.2".into()),
        abr: Some(128.0),
        ..video(id, "avc1.640033", height, 30.0, kbps)
    }
}

fn info(formats: Vec<Format>, seconds: f64) -> VideoInfo {
    VideoInfo {
        id: "abc".into(),
        title: "Test".into(),
        url: "https://example.com/watch?v=abc".into(),
        thumbnail: None,
        duration: Some(seconds),
        channel: None,
        view_count: None,
        extractor: None,
        formats,
        with_account: false,
        live: None,
        subtitles: Vec::new(),
        chat: None,
        saved: None,
        sizes: Default::default(),
    }
}

/// What YouTube typically offers for a 4K upload: H.264 up to 1080p, VP9 and AV1 above.
fn youtube_4k() -> VideoInfo {
    info(
        vec![
            Format {
                abr: Some(96.0),
                ..muxed("18", 360, 500.0)
            },
            video("136", "avc1.4d401f", 720, 30.0, 1500.0),
            video("137", "avc1.640028", 1080, 30.0, 4000.0),
            video("248", "vp09.00.40.08", 1080, 30.0, 2500.0),
            video("313", "vp09.00.50.08", 2160, 30.0, 18000.0),
            video("401", "av01.0.12M.08", 2160, 30.0, 12000.0),
            audio("140", "mp4a.40.2", 129.0),
            audio("251", "opus", 160.0),
            audio("251-drc", "opus", 165.0),
        ],
        600.0,
    )
}

/// What X (Twitter) offers: H.264 with AAC in one file, all the way up to 4K.
fn x_4k() -> VideoInfo {
    info(
        vec![
            muxed("http-832", 480, 832.0),
            muxed("http-2176", 720, 2176.0),
            muxed("http-10368", 2160, 10368.0),
        ],
        600.0,
    )
}

fn preset(target: TargetId) -> PresetChoice {
    PresetChoice {
        target,
        quality: None,
        tier: None,
        music_format: MusicFormat::Mp3,
        subtitles: None,
        chat: None,
    }
}

fn plan_with(video: &VideoInfo, choice: PresetChoice, caps: &Capabilities) -> Recipe {
    plan(video, &Choice::Preset(choice), None, caps).unwrap()
}

fn plan_for(video: &VideoInfo, target: TargetId) -> Recipe {
    plan_with(video, preset(target), &Capabilities::BASIC)
}

fn nvidia() -> Capabilities {
    Capabilities {
        encoders: Encoders {
            h264: Hardware::Nvenc,
            hevc: Hardware::Nvenc,
        },
        ..Capabilities::BASIC
    }
}

fn copied(recipe: &Recipe) -> bool {
    !recipe.encodes_video()
}

fn encoded_to(recipe: &Recipe) -> Option<VideoCodec> {
    recipe.video_encode().map(|encode| encode.codec)
}

fn strings(args: Vec<std::ffi::OsString>) -> Vec<String> {
    args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

// ── Watch ───────────────────────────────────────────────────────────────

#[test]
fn apple_copies_4k_h264_on_x_but_converts_youtube_4k_to_hevc() {
    let x = plan_with(&x_4k(), preset(TargetId::Apple), &nvidia());
    assert!(copied(&x));
    assert_eq!(x.format_selector(), "http-10368");
    assert_eq!(x.container, Container::Mp4);
    assert_eq!(x.badge, Badge::Lossless);

    let youtube = plan_with(&youtube_4k(), preset(TargetId::Apple), &nvidia());
    assert_eq!(youtube.output_lines(), Some(2160));
    assert_eq!(encoded_to(&youtube), Some(VideoCodec::Hevc));
    assert_eq!(youtube.video_encode().map(|e| e.hardware), Some(Hardware::Nvenc));
    assert_eq!(youtube.audio_codec(), Some(AudioCodec::Aac));
    assert_eq!(youtube.badge, Badge::Converts);
}

#[test]
fn apple_copies_h264_when_a_lower_quality_has_it() {
    let recipe = plan_with(
        &youtube_4k(),
        PresetChoice {
            quality: Some(1080),
            ..preset(TargetId::Apple)
        },
        &nvidia(),
    );
    assert!(copied(&recipe));
    assert_eq!(recipe.format_selector(), "137+140");
}

#[test]
fn apple_hevc_is_tagged_hvc1_whether_converted_or_copied() {
    let converted = plan_with(&youtube_4k(), preset(TargetId::Apple), &nvidia());
    let args = strings(ffmpeg::convert_args(
        &converted,
        Path::new("in.mkv"),
        Path::new("out.mp4"),
        Pass::Only,
    ));
    assert!(args.windows(2).any(|pair| pair == ["-tag:v", "hvc1"]));
    assert!(args.windows(2).any(|pair| pair == ["-c:v", "hevc_nvenc"]));

    let hevc_source = info(
        vec![
            video("hev", "hvc1.1.6.L150", 2160, 30.0, 9000.0),
            audio("a", "mp4a.40.2", 128.0),
        ],
        60.0,
    );
    let copied = plan_for(&hevc_source, TargetId::Apple);
    assert!(!copied.needs_ffmpeg_pass());
    let yt_dlp = args::recipe_args(&copied);
    assert!(yt_dlp.iter().any(|arg| arg.contains("-tag:v hvc1")));
    // A whole video from YouTube is fetched in pieces, four at once.
    assert!(yt_dlp
        .windows(2)
        .any(|pair| pair == ["--extractor-args", "youtube:formats=dashy"]));
}

#[test]
fn apple_without_a_hardware_hevc_encoder_uses_hardware_h264() {
    let caps = Capabilities {
        encoders: Encoders {
            h264: Hardware::Qsv,
            hevc: Hardware::Software,
        },
        ..Capabilities::BASIC
    };
    let recipe = plan_with(&youtube_4k(), preset(TargetId::Apple), &caps);
    assert_eq!(encoded_to(&recipe), Some(VideoCodec::H264));
    assert_eq!(recipe.video_encode().map(|e| e.hardware), Some(Hardware::Qsv));
}

#[test]
fn windows_copies_vp9_only_when_the_extension_is_installed() {
    let without = plan_for(&youtube_4k(), TargetId::Windows);
    assert_eq!(without.output_lines(), Some(2160));
    assert_eq!(encoded_to(&without), Some(VideoCodec::H264));

    let caps = Capabilities {
        windows: WindowsCodecs {
            vp9: true,
            ..WindowsCodecs::default()
        },
        ..Capabilities::BASIC
    };
    let with = plan_with(&youtube_4k(), preset(TargetId::Windows), &caps);
    assert!(copied(&with));
    assert_eq!(with.format_selector(), "313+251");
    assert_eq!(with.container, Container::Webm);
    assert!(with.hints.iter().any(|hint| hint.text.contains("this PC has")));

    // Only AAC beside the VP9 (a YouTube live stream from its start): the
    // sound becomes Opus, which WebM takes, not AAC, which it doesn't.
    let mut aac_only = youtube_4k();
    aac_only
        .formats
        .retain(|format| format.kind != FormatKind::Audio || format.acodec.as_deref() != Some("opus"));
    let with = plan_with(&aac_only, preset(TargetId::Windows), &caps);
    assert_eq!(with.container, Container::Webm);
    assert!(matches!(
        with.audio.as_ref().map(|audio| &audio.action),
        Some(AudioAction::Encode {
            codec: AudioCodec::Opus,
            ..
        })
    ));
}

#[test]
fn android_stops_at_1080p_unless_asked_and_copies_vp9_above() {
    let best = plan_for(&youtube_4k(), TargetId::Android);
    assert!(copied(&best));
    assert_eq!(best.format_selector(), "137+140");

    let uhd = plan_with(
        &youtube_4k(),
        PresetChoice {
            quality: Some(2160),
            ..preset(TargetId::Android)
        },
        &Capabilities::BASIC,
    );
    assert!(copied(&uhd));
    assert_eq!(uhd.format_selector(), "313+140");
    assert_eq!(uhd.container, Container::Mp4);
}

#[test]
fn tv_converts_4k_vp9_to_hevc_and_copies_x() {
    let youtube = plan_with(&youtube_4k(), preset(TargetId::Tv), &nvidia());
    assert_eq!(encoded_to(&youtube), Some(VideoCodec::Hevc));
    assert!(copied(&plan_for(&x_4k(), TargetId::Tv)));
}

#[test]
fn a_lower_quality_takes_the_closest_stream_below_and_never_upscales() {
    let at_900 = plan_with(
        &x_4k(),
        PresetChoice {
            quality: Some(900),
            ..preset(TargetId::Windows)
        },
        &Capabilities::BASIC,
    );
    assert_eq!(at_900.format_selector(), "http-2176");
    assert!(copied(&at_900));

    // Asking for more than the video has gets the best it has, as it is.
    let small = info(vec![muxed("only", 480, 900.0)], 60.0);
    let at_2160 = plan_with(
        &small,
        PresetChoice {
            quality: Some(2160),
            ..preset(TargetId::Apple)
        },
        &Capabilities::BASIC,
    );
    assert_eq!(at_2160.output_lines(), Some(480));
    assert!(copied(&at_2160));
}

#[test]
fn the_quality_row_lists_only_resolutions_the_video_has_without_tiny_ones() {
    let (kind, values) = quality_options(
        &youtube_4k(),
        &Choice::Preset(preset(TargetId::Windows)),
        &Capabilities::BASIC,
    );
    assert_eq!(kind, QualityKind::Resolution);
    assert_eq!(values, vec![2160, 1080, 720, 360]);

    let (kind, _) = quality_options(
        &youtube_4k(),
        &Choice::Preset(preset(TargetId::Max)),
        &Capabilities::BASIC,
    );
    assert_eq!(kind, QualityKind::None);
    let (kind, _) = quality_options(
        &youtube_4k(),
        &Choice::Preset(preset(TargetId::Discord)),
        &Capabilities::BASIC,
    );
    assert_eq!(kind, QualityKind::None);
}

#[test]
fn every_option_in_the_quality_row_has_a_size() {
    let summary = explain::summarize(
        &x_4k(),
        &Choice::Preset(preset(TargetId::Windows)),
        None,
        &Capabilities::BASIC,
    )
    .unwrap();
    assert_eq!(summary.options.len(), 3);
    assert!(summary
        .options
        .iter()
        .all(|option| option.estimated_size.is_some() && !option.converts));
    assert_eq!(summary.resolved, Some(2160));
    assert_eq!(summary.badge, Badge::Lossless);
}

#[test]
fn a_conversion_says_roughly_how_long_it_takes() {
    let summary = explain::summarize(&youtube_4k(), &Choice::Preset(preset(TargetId::Apple)), None, &nvidia()).unwrap();
    // Ten minutes of 4K on the GPU: a few minutes, not seconds or hours.
    let seconds = summary.convert_seconds.unwrap();
    assert!((60.0..1200.0).contains(&seconds), "{seconds}");
    assert!(summary.detail.contains("NVIDIA"));
}

// ── Audio ───────────────────────────────────────────────────────────────

#[test]
fn music_converts_to_mp3_near_the_source_bitrate_and_copies_aac_for_m4a() {
    let mp3 = plan_for(&youtube_4k(), TargetId::Music);
    assert!(mp3.video.is_none());
    assert_eq!(mp3.container, Container::Mp3);
    assert!(matches!(
        mp3.audio.as_ref().map(|a| &a.action),
        Some(AudioAction::Encode {
            codec: AudioCodec::Mp3,
            bitrate_kbps: Some(192),
            ..
        })
    ));

    let m4a = plan_with(
        &youtube_4k(),
        PresetChoice {
            music_format: MusicFormat::M4a,
            ..preset(TargetId::Music)
        },
        &Capabilities::BASIC,
    );
    assert_eq!(m4a.format_selector(), "140");
    assert!(!m4a.encodes_audio());
    assert_eq!(m4a.badge, Badge::Lossless);
}

#[test]
fn music_says_when_a_bitrate_is_higher_than_the_source() {
    let recipe = plan_with(
        &youtube_4k(),
        PresetChoice {
            quality: Some(320),
            ..preset(TargetId::Music)
        },
        &Capabilities::BASIC,
    );
    assert!(recipe.hints.iter().any(|hint| hint.text.contains("larger, not better")));
    let args = args::recipe_args(&recipe);
    assert!(args.windows(2).any(|pair| pair == ["--audio-quality", "320K"]));
}

#[test]
fn best_audio_keeps_the_original_track() {
    let recipe = plan_for(&youtube_4k(), TargetId::Original);
    assert_eq!(recipe.format_selector(), "251");
    assert_eq!(recipe.container, Container::Opus);
    assert!(!recipe.encodes_audio());
}

#[test]
fn audio_only_sources_ignore_video_presets() {
    let podcast = info(vec![audio("a", "mp4a.40.2", 128.0)], 3600.0);
    let recipe = plan_for(&podcast, TargetId::Apple);
    assert!(recipe.video.is_none());
    assert_eq!(recipe.container, Container::M4a);
}

// ── Max quality ─────────────────────────────────────────────────────────

#[test]
fn max_quality_copies_the_best_streams_and_warns_it_may_not_play() {
    let recipe = plan_for(&youtube_4k(), TargetId::Max);
    assert_eq!(recipe.format_selector(), "401+251");
    assert_eq!(recipe.container, Container::Webm);
    assert_eq!(recipe.badge, Badge::MayNotPlay);
}

// ── Share ───────────────────────────────────────────────────────────────

fn share(video: &VideoInfo, target: TargetId, tier: TierId, trim: Option<Trim>) -> Recipe {
    plan(
        video,
        &Choice::Preset(PresetChoice {
            tier: Some(tier),
            ..preset(target)
        }),
        trim,
        &nvidia(),
    )
    .unwrap()
}

fn youtube_without_360p() -> VideoInfo {
    let mut video = youtube_4k();
    video.formats.retain(|format| format.id != "18");
    video
}

#[test]
fn share_copies_the_small_original_when_it_is_as_sharp_as_an_encode_would_be() {
    let recipe = share(&youtube_4k(), TargetId::Discord, TierId::DiscordNitroBasic, None);
    assert_eq!(recipe.format_selector(), "18");
    assert!(copied(&recipe));
}

/// X's formats for a 20-second clip.
fn x_clip() -> VideoInfo {
    let formats = x_4k()
        .formats
        .into_iter()
        .map(|format| Format {
            filesize: format.tbr.map(|kbps| (kbps * 1000.0 / 8.0 * 20.0) as u64),
            ..format
        })
        .collect();
    info(formats, 20.0)
}

#[test]
fn share_copies_an_original_that_already_fits() {
    // The 720p file is 5.4 MB; the 4K one, 26 MB, is too large.
    let recipe = share(&x_clip(), TargetId::Discord, TierId::DiscordFree, None);
    assert!(copied(&recipe));
    assert_eq!(recipe.format_selector(), "http-2176");
    assert_eq!(recipe.size_limit, Some(20_000_000));
    assert_eq!(recipe.badge, Badge::Lossless);
    // If the copy comes out over the limit after all, it is converted.
    assert!(recipe
        .fallback
        .as_ref()
        .is_some_and(|fallback| fallback.encodes_video()));
}

#[test]
fn share_encodes_to_a_budget_that_fits_and_drops_resolution_before_quality() {
    // Ten minutes into 50 MB: about 630 kbit/s all told. (Without YouTube's
    // small 360p file, which would fit as it is.)
    let recipe = share(
        &youtube_without_360p(),
        TargetId::Discord,
        TierId::DiscordNitroBasic,
        None,
    );
    let encode = recipe.video_encode().expect("nothing fits as it is");
    assert_eq!(encode.codec, VideoCodec::H264);
    let estimated = recipe.estimated_size().unwrap();
    assert!(estimated <= 50_000_000, "estimated {estimated}");
    assert!(
        estimated > 40_000_000,
        "the budget should be used, estimated {estimated}"
    );
    // Too few bits for 1080p or 720p to look clean.
    assert!(recipe.output_lines().unwrap() <= 480);
    assert_eq!(recipe.badge, Badge::SizeOptimized);
    assert!(matches!(
        recipe.audio.as_ref().map(|a| &a.action),
        Some(AudioAction::Encode {
            codec: AudioCodec::Aac,
            bitrate_kbps: Some(96),
            stereo: true,
            ..
        })
    ));
    assert!(recipe.blocked.is_none());
    let args = strings(ffmpeg::convert_args(
        &recipe,
        Path::new("in.mkv"),
        Path::new("out.mp4"),
        Pass::Only,
    ));
    assert!(args.windows(2).any(|pair| pair == ["-movflags", "+faststart"]));
}

#[test]
fn share_budgets_for_the_trimmed_length() {
    let trim = Some(Trim { start: 60.0, end: 90.0 });
    let recipe = share(&youtube_4k(), TargetId::Whatsapp, TierId::WhatsappHd, trim);
    assert_eq!(recipe.duration, Some(30.0));
    // 30 seconds into 100 MB would allow far more, but WhatsApp HD shows 720p.
    assert_eq!(recipe.output_lines(), Some(720));
    let whole = share(&youtube_4k(), TargetId::Discord, TierId::DiscordFree, None);
    let part = share(&youtube_4k(), TargetId::Discord, TierId::DiscordFree, trim);
    // The same limit buys a sharper picture for a shorter part.
    assert!(part.output_lines() > whole.output_lines());
    assert!(part.estimated_size().unwrap() <= 20_000_000);
}

#[test]
fn share_blocks_a_video_too_long_to_look_decent() {
    let long = info(youtube_4k().formats, 3.0 * 3600.0);
    let recipe = share(&long, TargetId::Discord, TierId::DiscordFree, None);
    assert!(recipe.blocked.as_deref().is_some_and(|reason| reason.contains("Trim")));
}

#[test]
fn share_respects_what_the_service_shows() {
    let whatsapp = share(&youtube_4k(), TargetId::Whatsapp, TierId::WhatsappStandard, None);
    assert!(whatsapp.output_lines().unwrap() <= 480);
    let social = share(&youtube_4k(), TargetId::Social, TierId::Shorts, None);
    assert_eq!(social.output_lines(), Some(1080));
    // A generous limit isn't filled for nothing.
    assert!(social.estimated_size().unwrap() < 600_000_000);
}

#[test]
fn a_share_file_over_the_limit_is_encoded_again_smaller() {
    let recipe = share(
        &youtube_without_360p(),
        TargetId::Discord,
        TierId::DiscordNitroBasic,
        None,
    );
    let before = recipe.video_encode().unwrap().bitrate_kbps.unwrap();
    let smaller = recipe.shrunk(55_000_000).unwrap();
    let after = smaller.video_encode().unwrap().bitrate_kbps.unwrap();
    assert!(f64::from(after) <= f64::from(before) * 50.0 * 0.95 / 55.0 + 1.0);
    // Nothing to lower on a copy.
    assert!(plan_for(&x_4k(), TargetId::Max).shrunk(1).is_none());
}

#[test]
fn share_on_the_cpu_runs_two_passes() {
    let recipe = plan(
        &youtube_4k(),
        &Choice::Preset(preset(TargetId::Discord)),
        None,
        &Capabilities::BASIC,
    )
    .unwrap();
    assert!(recipe.video_encode().unwrap().two_pass());
    let first = strings(ffmpeg::convert_args(
        &recipe,
        Path::new("in.mkv"),
        Path::new("out.mp4"),
        Pass::First { log: Path::new("log") },
    ));
    assert!(first.windows(2).any(|pair| pair == ["-pass", "1"]));
    assert!(first.contains(&"-an".to_string()));
    assert!(!first.contains(&"out.mp4".to_string()));
}

#[test]
fn bits_per_pixel_pick_a_smaller_cleaner_picture() {
    // 2.5 Mbit/s: blocky at 1080p60, clean at 720p30.
    let rung = share::pick_rung(2500.0, 1080, 60.0, 16.0 / 9.0, 1080).unwrap();
    assert_eq!((rung.lines, rung.fps), (720, 30.0));
    // Plenty of bits: the source as it is, never above it.
    let rung = share::pick_rung(50_000.0, 720, 30.0, 16.0 / 9.0, 1080).unwrap();
    assert_eq!((rung.lines, rung.fps), (720, 30.0));
    // Hopeless.
    assert!(share::pick_rung(20.0, 1080, 30.0, 16.0 / 9.0, 1080).is_none());
}

// ── Trims and Custom ────────────────────────────────────────────────────

#[test]
fn a_cut_copied_stream_is_smart_cut_in_its_own_codec() {
    let trim = Some(Trim { start: 0.0, end: 60.0 });
    let whole = plan_for(&x_4k(), TargetId::Windows).estimated_size().unwrap();
    let part = plan(&x_4k(), &Choice::Preset(preset(TargetId::Windows)), trim, &nvidia()).unwrap();
    // The stream is copied; only the frames at the cuts are re-encoded, in H.264 on the GPU.
    assert!(copied(&part));
    assert!(part.smart_cut() && part.needs_ffmpeg_pass());
    let edges = part.cut_encode.as_ref().unwrap();
    assert_eq!((edges.codec, edges.hardware), (VideoCodec::H264, Hardware::Nvenc));
    assert_eq!(part.badge, Badge::Lossless);
    // Its share of the whole, and the padding the cut leaves where room was
    // kept for the MP4's index: 60 s of 30 fps video with AAC.
    let padding = crate::ffmpeg::smart_cut::Index::unused(60 * 30, 60.0);
    assert_eq!(part.estimated_size().unwrap(), whole / 10 + padding);
    // Its fallback re-encodes the whole part, the same way.
    let full = part.with_full_cut();
    assert_eq!(encoded_to(&full), Some(VideoCodec::H264));
    assert!(!full.smart_cut());
    // The summary says so, and that it takes seconds.
    let summary = explain::summarize(&x_4k(), &Choice::Preset(preset(TargetId::Windows)), trim, &nvidia()).unwrap();
    assert!(
        summary.detail.contains("only the frames at the cuts are re-encoded"),
        "{}",
        summary.detail
    );
    assert!(summary.convert_seconds.is_some_and(|seconds| seconds < 30.0));
    // Without a trim nothing changes.
    assert!(!plan_for(&x_4k(), TargetId::Windows).smart_cut());
}

// ── Sizes measured on the video ─────────────────────────────────────────

/// `video` with its streams readable, as a lookup gives them.
fn readable(mut video: VideoInfo) -> VideoInfo {
    for format in &mut video.formats {
        format.stream_url = Some(format!("https://example.com/{}", format.id));
    }
    video
}

#[test]
fn a_clip_goes_by_its_bytes_and_its_ends_once_measured() {
    use super::sizes::{SizeFacts, Span};
    let trim = Some(Trim {
        start: 120.0,
        end: 150.0,
    });
    let choice = Choice::Preset(PresetChoice {
        quality: Some(1080),
        ..preset(TargetId::Windows)
    });
    let mut video = readable(youtube_4k());
    let before = plan(&video, &choice, trim, &nvidia()).unwrap();
    assert!(before.smart_cut());
    // The part's bytes of both streams, and what the cut's ends add, are wanted.
    let wanted = video.sizes.wanted(&before, &video);
    assert_eq!(wanted.parts, ["137", "140"]);
    assert_eq!(wanted.span, Some(Span::of(trim)));
    assert!(wanted.cut_ends && !wanted.encode);

    let mut facts = SizeFacts::default();
    facts.add_part("137", Span::of(trim), 30_000_000);
    facts.add_part("140", Span::of(trim), 500_000);
    facts.add_cut_ends("137", Span::of(trim), 4_000_000);
    video.sizes.merge(&facts);
    let after = plan(&video, &choice, trim, &nvidia()).unwrap();
    let padding = crate::ffmpeg::smart_cut::Index::unused(30 * 30, 30.0);
    assert_eq!(after.estimated_size(), Some(30_000_000 + 4_000_000 + padding + 500_000));
    // What yt-dlp fetches: the streams' bytes, without what the cut adds.
    assert_eq!(after.estimated_source_bytes(), Some(30_500_000));
    assert!(video.sizes.wanted(&after, &video).is_empty());
    // Another part is another measurement.
    let other = Some(Trim {
        start: 200.0,
        end: 230.0,
    });
    let elsewhere = plan(&video, &choice, other, &nvidia()).unwrap();
    assert_eq!(video.sizes.wanted(&elsewhere, &video).parts, ["137", "140"]);
}

#[test]
fn only_what_can_be_read_and_isnt_stated_is_measured() {
    let trim = Some(Trim { start: 0.0, end: 60.0 });
    let choice = Choice::Preset(preset(TargetId::Windows));
    // Streams yt-dlp gave no link for can't be read.
    let unreadable = youtube_4k();
    let recipe = plan(&unreadable, &choice, trim, &nvidia()).unwrap();
    assert!(unreadable.sizes.wanted(&recipe, &unreadable).is_empty());
    // A whole video's stated sizes are its sizes: nothing to measure but a conversion.
    let video = readable(x_4k());
    let whole = plan(&video, &choice, None, &nvidia()).unwrap();
    assert!(video.sizes.wanted(&whole, &video).is_empty());
    // A stream whose size is only its bitrate times its length (Twitch's) is measured whole.
    let mut twitch = readable(x_4k());
    for format in &mut twitch.formats {
        format.segmented = true;
        format.filesize_estimated = true;
    }
    let whole = plan(&twitch, &Choice::Preset(preset(TargetId::Max)), None, &nvidia()).unwrap();
    assert_eq!(twitch.sizes.wanted(&whole, &twitch).parts, ["http-10368"]);
    // Never a stream as it airs.
    let mut live = readable(x_4k());
    live.live = Some(crate::ytdlp::metadata::Live {
        status: crate::ytdlp::metadata::LiveStatus::Live,
        since: None,
        viewers: None,
        from_start: None,
    });
    assert!(live.sizes.wanted(&whole, &live).is_empty());
}

#[test]
fn a_measured_conversion_is_one_size_and_lends_its_measure_to_the_others() {
    use super::sizes::SizeFacts;
    let mut video = readable(youtube_4k());
    let tv = Choice::Preset(preset(TargetId::Tv));
    let recipe = plan(&video, &tv, None, &nvidia()).unwrap();
    assert!(recipe.encodes_video());
    let guess = recipe.estimate().unwrap();
    assert!(guess.is_uncertain());
    assert!(video.sizes.wanted(&recipe, &video).encode);
    // It came to one and a half times the rule of thumb.
    let rule = recipe.rule_of_thumb_video().unwrap();
    let mut facts = SizeFacts::default();
    facts.add_encode(&recipe, 1.5);
    video.sizes.merge(&facts);
    let measured = plan(&video, &tv, None, &nvidia()).unwrap();
    assert!(measured.measured.encode_exact);
    let estimate = measured.estimate().unwrap();
    assert!(!estimate.is_uncertain());
    let audio = guess.likely - rule as u64;
    assert_eq!(estimate.likely, (rule * 1.5) as u64 + audio);
    assert!(!video.sizes.wanted(&measured, &video).encode);
    // The same encoder on a part of it goes by it too, a little less sure.
    let part = plan(&video, &tv, Some(Trim { start: 0.0, end: 60.0 }), &nvidia()).unwrap();
    assert!(part.encodes_video());
    assert_eq!(part.measured.encode, Some(1.5));
    assert!(!part.measured.encode_exact);
    let narrower = part.estimate().unwrap();
    let guessed = plan(&youtube_4k(), &tv, Some(Trim { start: 0.0, end: 60.0 }), &nvidia())
        .unwrap()
        .estimate()
        .unwrap();
    assert!(narrower.high - narrower.low < (guessed.high - guessed.low) / 2);
}

#[test]
fn cuts_that_cant_be_smart_are_converted() {
    let trim = Some(Trim { start: 10.0, end: 20.0 });
    // YouTube's 4K is VP9 or AV1: Max quality converts it when cut.
    let max = plan(&youtube_4k(), &Choice::Preset(preset(TargetId::Max)), trim, &nvidia()).unwrap();
    assert!(max.encodes_video() && !max.smart_cut());
    assert!(max
        .reason
        .as_deref()
        .unwrap()
        .contains("can only be cut on the exact frames by converting it"));
    // Uncut, it is copied.
    assert!(copied(&plan_for(&youtube_4k(), TargetId::Max)));
}

#[test]
fn share_copies_a_cut_original_that_fits() {
    let trim = Some(Trim { start: 0.0, end: 10.0 });
    let recipe = share(&x_4k(), TargetId::Discord, TierId::DiscordNitro, trim);
    assert!(copied(&recipe) && recipe.smart_cut());
    // Converted after all if it comes out over the limit.
    assert!(recipe.fallback.is_some());
}

#[test]
fn a_cut_for_apple_is_re_encoded_in_full_and_counts_from_the_start() {
    let trim = Some(Trim { start: 10.0, end: 20.0 });
    let recipe = plan(&x_4k(), &Choice::Preset(preset(TargetId::Apple)), trim, &nvidia()).unwrap();
    assert_eq!(encoded_to(&recipe), Some(VideoCodec::H264));
    let yt_dlp = args::recipe_args(&recipe);
    assert!(yt_dlp.iter().any(|arg| arg.contains("-copyts -start_at_zero")));
    // A second past the end, for the frames the last ones are decoded from.
    assert!(yt_dlp.windows(2).any(|pair| pair == ["--download-sections", "*10-21"]));
    // ffmpeg fetches a part from the stream's address: YouTube's plain files, not pieces.
    assert!(!yt_dlp.iter().any(|arg| arg.contains("dashy")));
}

#[test]
fn custom_defaults_copy_everything_and_containers_force_codecs() {
    let keep = plan(
        &youtube_4k(),
        &Choice::Custom(ManualOptions::default()),
        None,
        &Capabilities::BASIC,
    )
    .unwrap();
    assert!(!keep.encodes_video() && !keep.encodes_audio());

    let mov = ManualOptions {
        container: Some(Container::Mov),
        ..ManualOptions::default()
    };
    let recipe = plan(&youtube_4k(), &Choice::Custom(mov), None, &Capabilities::BASIC).unwrap();
    assert_eq!(encoded_to(&recipe), Some(VideoCodec::H264));
}

#[test]
fn empty_cuts_and_unknown_streams_are_rejected() {
    let empty = Some(Trim { start: 5.0, end: 5.0 });
    assert!(plan(
        &youtube_4k(),
        &Choice::Preset(preset(TargetId::Max)),
        empty,
        &Capabilities::BASIC
    )
    .is_err());
    let unknown = ManualOptions {
        video_format: Some("nope".into()),
        ..ManualOptions::default()
    };
    assert!(plan(&youtube_4k(), &Choice::Custom(unknown), None, &Capabilities::BASIC).is_err());
}

#[test]
fn plain_described_streams_beat_segmented_copies() {
    let mut hls = video("hls-1080", "avc1.640028", 1080, 30.0, 5000.0);
    hls.segmented = true;
    let video = info(
        vec![
            hls,
            video("137", "avc1.640028", 1080, 30.0, 4000.0),
            audio("140", "mp4a.40.2", 128.0),
        ],
        60.0,
    );
    assert_eq!(plan_for(&video, TargetId::Windows).format_selector(), "137+140");
}

// ── Size estimates ──────────────────────────────────────────────────────

/// One video stream as the calibration sources had it, ten minutes long.
fn source(vcodec: &str, height: u32, fps: f64, kbps: f64) -> VideoInfo {
    info(
        vec![video("v", vcodec, height, fps, kbps), audio("a", "mp4a.40.2", 128.0)],
        600.0,
    )
}

#[test]
fn quality_conversion_estimates_cover_what_test_encodes_measured() {
    // Measured on 2026-11-17 with Siphon's settings (docs/decisions.md#size-estimates).
    let cases = [
        // Big Buck Bunny, VP9 4K60 at 12.2 Mbit/s → H.264 on NVENC: 33.2 Mbit/s.
        (
            source("vp09.00.51.08", 2160, 60.0, 12_170.0),
            TargetId::Windows,
            None,
            33_200.0,
        ),
        // A talking head, AV1 1080p30 at 0.28 Mbit/s → H.264 on NVENC: 0.4 Mbit/s.
        (
            source("av01.0.08M.08", 1080, 30.0, 283.0),
            TargetId::Windows,
            None,
            400.0,
        ),
    ];
    for (video, target, quality, measured_kbps) in cases {
        let recipe = plan_with(
            &video,
            PresetChoice {
                quality,
                ..preset(target)
            },
            &nvidia(),
        );
        assert!(recipe.encodes_video());
        let estimate = recipe.estimate().unwrap();
        let audio = recipe.audio.as_ref().map_or(0.0, |audio| match audio.action {
            AudioAction::Encode { bitrate_kbps, .. } => f64::from(bitrate_kbps.unwrap_or(0)),
            AudioAction::Copy => 128.0,
        });
        let measured = ((measured_kbps + audio) * 1000.0 / 8.0 * 600.0) as u64;
        assert!(
            estimate.low <= measured && measured <= estimate.high,
            "{target:?} {quality:?}: measured {measured} outside {estimate:?}"
        );
        assert!(estimate.is_uncertain());
    }
}

#[test]
fn copies_and_size_targeted_encodes_get_one_number() {
    let copy = plan_for(&x_4k(), TargetId::Windows).estimate().unwrap();
    assert!(!copy.is_uncertain());
    let share = share(&youtube_without_360p(), TargetId::Discord, TierId::DiscordFree, None)
        .estimate()
        .unwrap();
    assert!(!share.is_uncertain());
    let summary = explain::summarize(&youtube_4k(), &Choice::Preset(preset(TargetId::Apple)), None, &nvidia()).unwrap();
    // The 4K conversion is a range; the 1080p copy option is not.
    assert!(summary.size_range.is_some());
    let hd = summary.options.iter().find(|option| option.value == 1080).unwrap();
    assert!(hd.size_range.is_none() && !hd.converts);
}

#[test]
fn trimmed_parts_give_up_on_a_stalled_connection() {
    let trim = Some(Trim { start: 10.0, end: 20.0 });
    let recipe = plan(
        &x_4k(),
        &Choice::Preset(preset(TargetId::Windows)),
        trim,
        &Capabilities::BASIC,
    )
    .unwrap();
    let yt_dlp = args::recipe_args(&recipe);
    assert!(yt_dlp
        .iter()
        .any(|arg| arg.starts_with("ffmpeg_i:") && arg.contains("-rw_timeout 20000000")));
}

#[test]
fn a_download_works_under_its_own_marker_and_drops_it_at_the_end() {
    let recipe = plan_for(&x_4k(), TargetId::Windows);
    let own = args::own_marker("1a2b3c4d-5e6f-7a8b");
    assert_eq!(own, ".siphon-1a2b3c4d");
    assert_eq!(
        args::output_template(&recipe, "%(title)s", &own),
        "%(title)s.siphon-1a2b3c4d.%(ext)s"
    );
    assert_eq!(args::finished_stem("Clip [x].siphon-1a2b3c4d"), "Clip [x]");
    assert_eq!(args::finished_stem("Clip [x].source"), "Clip [x]");
    // A title that merely contains the word is left alone.
    assert_eq!(args::finished_stem("My .siphon-day out"), "My .siphon-day out");
    assert_eq!(args::finished_stem("Clip"), "Clip");
}

#[test]
fn audio_files_get_tags_and_cover_art_and_videos_are_left_alone() {
    let has = |recipe: &Recipe, flag: &str| args::recipe_args(recipe).iter().any(|arg| arg == flag);
    let mp3 = plan_for(&youtube_4k(), TargetId::Music);
    assert!(has(&mp3, "--embed-metadata") && has(&mp3, "--embed-thumbnail"));
    let original = plan_for(&youtube_4k(), TargetId::Original);
    assert!(has(&original, "--embed-metadata") && has(&original, "--embed-thumbnail"));
    // WAV takes tags but no cover: yt-dlp would fail the download.
    let mut wav = mp3.clone();
    wav.container = Container::Wav;
    assert!(has(&wav, "--embed-metadata") && !has(&wav, "--embed-thumbnail"));
    // A video is not rewritten once more just for tags.
    let video = plan_for(&youtube_4k(), TargetId::Windows);
    assert!(!has(&video, "--embed-metadata") && !has(&video, "--embed-thumbnail"));
}

// ── Subtitles ───────────────────────────────────────────────────────────

fn with_subtitles(
    mut video: VideoInfo,
    tracks: &[(&str, crate::ytdlp::subtitles::SubtitleKind, &[&str])],
) -> VideoInfo {
    video.subtitles = tracks
        .iter()
        .map(|(key, kind, exts)| crate::ytdlp::subtitles::SubtitleTrack {
            key: key.to_string(),
            lang: crate::ytdlp::subtitles::language_tag(key),
            name: None,
            kind: *kind,
            exts: exts.iter().map(|ext| ext.to_string()).collect(),
        })
        .collect();
    video
}

fn wanting(target: TargetId, language: &str) -> PresetChoice {
    PresetChoice {
        subtitles: Some(language.into()),
        ..preset(target)
    }
}

#[test]
fn subtitles_go_inside_a_file_to_watch_and_beside_one_to_edit() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    use subtitles::Delivery;
    let video = with_subtitles(
        x_4k(),
        &[("en", Uploaded, &["srt", "vtt"]), ("el", Automatic, &["srt"])],
    );

    // Watch on Windows: a track in an MKV, the file type Windows' players show
    // subtitles from. The streams are still copied, in Siphon's own pass.
    let watch = plan_with(&video, wanting(TargetId::Windows, "en"), &Capabilities::BASIC);
    let step = watch.subtitles.as_ref().unwrap();
    assert_eq!(
        (step.key.as_str(), step.delivery, step.styled),
        ("en", Delivery::Embed, false)
    );
    assert_eq!(step.title, "English");
    assert_eq!(watch.container, Container::Mkv);
    assert!(watch
        .hints
        .iter()
        .any(|hint| hint.text.contains("In MKV rather than MP4")));
    assert!(copied(&watch) && watch.needs_ffmpeg_pass() && watch.adds_only_subtitles());
    assert_eq!(watch.badge, Badge::Lossless);
    // TVs read them from MKV too; Apple devices and Android keep MP4's own timed text.
    let container = |target| plan_with(&video, wanting(target, "en"), &Capabilities::BASIC).container;
    assert_eq!(container(TargetId::Tv), Container::Mkv);
    assert_eq!(container(TargetId::Apple), Container::Mp4);
    assert_eq!(container(TargetId::Android), Container::Mp4);
    // Without subtitles, nothing changes.
    assert_eq!(plan_for(&video, TargetId::Windows).container, Container::Mp4);
    // Speech recognition is marked as machine-made, and says so.
    let greek = plan_with(&video, wanting(TargetId::Windows, "el"), &Capabilities::BASIC);
    assert!(greek.subtitles.as_ref().unwrap().automatic);
    assert!(greek.hints.iter().any(|hint| hint.text.contains("speech recognition")));

    // Nothing for a sound file, nor in a language the video lacks.
    for target in [TargetId::Music, TargetId::Original] {
        assert_eq!(
            plan_with(&video, wanting(target, "en"), &Capabilities::BASIC).subtitles,
            None
        );
    }
    let french = plan_with(&video, wanting(TargetId::Windows, "fr"), &Capabilities::BASIC);
    assert_eq!(french.subtitles, None);
    assert!(!french.needs_ffmpeg_pass());
    // And none when none are asked for.
    assert_eq!(plan_for(&video, TargetId::Windows).subtitles, None);
}

#[test]
fn max_quality_keeps_styled_subtitles_in_matroska_but_a_clip_cant() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    let video = with_subtitles(x_4k(), &[("ja", Uploaded, &["ass", "srt"])]);
    let max = plan_with(&video, wanting(TargetId::Max, "ja"), &Capabilities::BASIC);
    assert_eq!(max.container, Container::Mkv);
    assert!(max.subtitles.as_ref().unwrap().styled);

    // So does Watch on Windows; Apple's MP4 can't, and says what is lost.
    let windows = plan_with(&video, wanting(TargetId::Windows, "ja"), &Capabilities::BASIC);
    assert!(windows.subtitles.as_ref().unwrap().styled);
    let apple = plan_with(&video, wanting(TargetId::Apple, "ja"), &Capabilities::BASIC);
    assert_eq!(apple.container, Container::Mp4);
    assert!(!apple.subtitles.as_ref().unwrap().styled);
    assert!(apple.hints.iter().any(|hint| hint.text.contains("an MP4 file")));

    // A clip is cut as SubRip, styling or not.
    let clip = plan(
        &video,
        &Choice::Preset(wanting(TargetId::Max, "ja")),
        Some(Trim { start: 10.0, end: 20.0 }),
        &Capabilities::BASIC,
    )
    .unwrap();
    assert!(!clip.subtitles.as_ref().unwrap().styled);
    assert!(clip.hints.iter().any(|hint| hint.text.contains("a clip")));
}

#[test]
fn custom_settings_take_subtitles_unless_they_make_audio() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    let video = with_subtitles(youtube_4k(), &[("en-orig", Automatic, &["srt"])]);
    let custom = |content: Content| {
        plan(
            &video,
            &Choice::Custom(ManualOptions {
                content,
                container: Some(Container::Webm),
                subtitles: Some("en".into()),
                ..ManualOptions::default()
            }),
            None,
            &Capabilities::BASIC,
        )
        .unwrap()
    };
    let webm = custom(Content::VideoAudio);
    assert_eq!(webm.subtitles.as_ref().map(|step| step.key.as_str()), Some("en-orig"));
    assert!(webm.hints.iter().any(|hint| hint.text.contains("speech recognition")));
    // The file type chosen is kept, and where it falls short is said.
    assert_eq!(webm.container, Container::Webm);
    assert!(webm.hints.iter().any(|hint| hint.text.contains("inside WebM files")));
    assert_eq!(custom(Content::AudioOnly).subtitles, None);
}

#[test]
fn a_stream_as_it_airs_gets_no_subtitles() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    let mut video = with_subtitles(x_4k(), &[("en", Uploaded, &["vtt"])]);
    video.live = Some(crate::ytdlp::metadata::Live {
        status: crate::ytdlp::metadata::LiveStatus::Live,
        since: None,
        viewers: None,
        from_start: None,
    });
    assert_eq!(
        plan_with(&video, wanting(TargetId::Windows, "en"), &Capabilities::BASIC).subtitles,
        None
    );
}

#[test]
fn share_draws_subtitles_into_the_picture_even_when_the_original_fits() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    use subtitles::Delivery;
    // The 480p original fits Discord's 20 MB as it is …
    let video = with_subtitles(
        info(vec![muxed("http-832", 480, 200.0)], 60.0),
        &[("en", Uploaded, &["srt"])],
    );
    let plain = plan_with(&video, preset(TargetId::Discord), &Capabilities::BASIC);
    assert!(copied(&plain) && plain.fallback.is_some());

    // … but drawing subtitles in needs the picture encoded: Share's own encode, still sized to fit.
    let burned = plan_with(&video, wanting(TargetId::Discord, "en"), &Capabilities::BASIC);
    assert_eq!(
        burned.subtitles.as_ref().map(|step| step.delivery),
        Some(Delivery::BurnIn)
    );
    assert!(burned.encodes_video());
    assert_eq!(burned.size_limit, Some(20_000_000));
    assert_eq!(burned.badge, Badge::SizeOptimized);
    assert_eq!(burned.fallback, None);
    assert!(burned.reason.as_deref().unwrap().contains("drawn into the picture"));
    assert!(burned
        .hints
        .iter()
        .any(|hint| hint.text.contains("Discord shows no subtitle tracks")));

    // The encode draws them, by the file's name, in both passes of a CPU encode.
    let args = strings(ffmpeg::convert_args_shifted(
        &burned,
        Path::new("in.mkv"),
        Path::new("out.mp4"),
        Pass::Only,
        None,
        Some(Path::new(r"C:\work\abc\subtitles.srt")),
        None,
    ));
    let filters = &args[args.iter().position(|arg| arg == "-vf").unwrap() + 1];
    assert!(filters.starts_with("subtitles=filename=subtitles.srt"), "{filters}");
    // No subtitle track: it's in the picture.
    assert!(!args.iter().any(|arg| arg == "-c:s"));

    // Styled subtitles are drawn with their styling.
    let styled = with_subtitles(
        info(vec![muxed("http-832", 480, 200.0)], 60.0),
        &[("ja", Uploaded, &["ass"])],
    );
    let styled = plan_with(&styled, wanting(TargetId::Discord, "ja"), &Capabilities::BASIC);
    assert!(styled.subtitles.as_ref().unwrap().styled);
}

#[test]
fn a_subtitle_track_goes_in_as_the_file_type_holds_it() {
    use crate::ytdlp::subtitles::SubtitleKind::*;
    let video = with_subtitles(x_4k(), &[("el", Uploaded, &["srt"])]);
    let args = |target| {
        let recipe = plan_with(&video, wanting(target, "el"), &Capabilities::BASIC);
        strings(ffmpeg::convert_args_shifted(
            &recipe,
            Path::new("in.mkv"),
            Path::new("out"),
            Pass::Only,
            None,
            Some(Path::new("subtitles.srt")),
            None,
        ))
        .join(" ")
    };
    // MP4 (Apple): its own timed text, named the MP4 way.
    let apple = args(TargetId::Apple);
    assert!(apple.contains("-i subtitles.srt"), "{apple}");
    assert!(
        apple.contains("-map 1:0 -c:s mov_text -tag:s tx3g -metadata:s:s:0 language=ell"),
        "{apple}"
    );
    assert!(apple.contains("-disposition:s:0 default"), "{apple}");
    // Matroska (Windows): SubRip, named the Matroska way.
    let windows = args(TargetId::Windows);
    assert!(windows.contains("-c:s srt -metadata:s:s:0 language=gre"), "{windows}");
    assert!(windows.contains("-metadata:s:s:0 title=Greek"), "{windows}");
}

#[test]
fn audio_of_an_unnamed_kind_is_called_what_it_is() {
    // TED (2026-11-20): H.264 video, audio whose codec the site doesn't name.
    let mut sound = audio("hls-audio", "none", 128.0);
    sound.acodec = None;
    sound.ext = "mp4".into();
    let video = info(vec![video("hls-1080", "avc1.640028", 480, 30.0, 1500.0), sound], 60.0);
    let recipe = plan_for(&video, TargetId::Windows);
    assert!(recipe.encodes_audio());
    assert_eq!(recipe.reason.as_deref(), Some("the site doesn’t say what kind it is"));
    let summary = explain::summarize(
        &video,
        &Choice::Preset(preset(TargetId::Windows)),
        None,
        &Capabilities::BASIC,
    )
    .unwrap();
    assert!(
        summary
            .detail
            .contains("only the audio is converted to AAC because the site doesn’t say"),
        "{}",
        summary.detail
    );
    assert!(!summary.detail.contains("audio audio"));
}

// ── Chat ────────────────────────────────────────────────────────────────

/// A Twitch VOD as yt-dlp lists it: HLS streams with sound in them, H.264 up to 1080p60.
fn twitch_vod() -> VideoInfo {
    let hls = |id: &str, height: u32, fps: f64, kbps: f64| Format {
        segmented: true,
        fps: Some(fps),
        ..muxed(id, height, kbps)
    };
    VideoInfo {
        extractor: Some("TwitchVod".into()),
        chat: Some(crate::chat::ChatSource::Vod {
            id: "2929613567".into(),
            channel: "greekgodx".into(),
        }),
        ..info(
            vec![
                hls("480p", 480, 30.0, 1453.0),
                hls("720p60", 720, 60.0, 3438.0),
                hls("1080p60", 1080, 60.0, 7777.0),
            ],
            3600.0,
        )
    }
}

fn with_chat(target: TargetId) -> PresetChoice {
    PresetChoice {
        chat: Some(crate::chat::Placement::DEFAULT),
        ..preset(target)
    }
}

#[test]
fn chat_is_drawn_in_by_encoding_the_video_as_fast_as_the_encoder_goes() {
    let without = plan_with(&twitch_vod(), preset(TargetId::Max), &nvidia());
    assert!(copied(&without) && without.chat.is_none());

    for target in [TargetId::Max, TargetId::Windows] {
        let recipe = plan_with(&twitch_vod(), with_chat(target), &nvidia());
        let encode = recipe.video_encode().expect("the picture is encoded");
        assert_eq!(
            (encode.codec, encode.hardware, encode.fast),
            (VideoCodec::H264, Hardware::Nvenc, true)
        );
        assert_eq!(
            recipe.container, without.container,
            "the same file type as without chat"
        );
        assert_eq!(recipe.badge, Badge::Converts);
        assert_eq!(recipe.reason.as_deref(), Some("the chat is drawn into the picture"));
        assert_eq!(recipe.chat.as_ref().unwrap().placement, crate::chat::Placement::DEFAULT);
        let summary = explain::summarize(&twitch_vod(), &Choice::Preset(with_chat(target)), None, &nvidia()).unwrap();
        assert!(
            summary.detail.contains("because the chat is drawn into the picture"),
            "{}",
            summary.detail
        );
        // About fourteen times real time for 1080p60 on NVENC's fastest setting.
        let seconds = summary.convert_seconds.unwrap();
        assert!((200.0..320.0).contains(&seconds), "{seconds}");
    }
}

#[test]
fn chat_goes_with_max_quality_and_watch_on_twitch_only() {
    for target in [TargetId::Music, TargetId::Original, TargetId::Discord] {
        let recipe = plan_with(&twitch_vod(), with_chat(target), &nvidia());
        assert!(recipe.chat.is_none(), "{target:?}");
    }
    // Only Twitch's chat is drawn in: elsewhere the plan is the same as without.
    let youtube = plan_with(&youtube_4k(), with_chat(TargetId::Windows), &nvidia());
    assert_eq!(youtube, plan_with(&youtube_4k(), preset(TargetId::Windows), &nvidia()));
}

#[test]
fn a_clip_with_chat_is_encoded_whole_rather_than_smart_cut() {
    let trim = Some(Trim {
        start: 600.0,
        end: 660.0,
    });
    let plain = plan(&twitch_vod(), &Choice::Preset(preset(TargetId::Max)), trim, &nvidia()).unwrap();
    assert!(plain.smart_cut());
    let recipe = plan(
        &twitch_vod(),
        &Choice::Preset(with_chat(TargetId::Max)),
        trim,
        &nvidia(),
    )
    .unwrap();
    assert!(!recipe.smart_cut() && recipe.cut_encode.is_none());
    assert_eq!(recipe.cut(), trim);
}

#[test]
fn the_chat_layer_is_read_from_standard_input_and_laid_over_the_picture() {
    let recipe = plan(
        &twitch_vod(),
        &Choice::Preset(with_chat(TargetId::Max)),
        Some(Trim {
            start: 600.0,
            end: 660.0,
        }),
        &nvidia(),
    )
    .unwrap();
    let layer = ffmpeg::ChatLayer {
        width: 422,
        height: 692,
        fps: 30.0,
        x: 1468,
        y: 324,
        start: 600.0,
        hd: true,
        end: Some(660.0),
        rate: None,
    };
    let args = strings(ffmpeg::convert_args_shifted(
        &recipe,
        Path::new("in.mp4"),
        Path::new("out.mp4"),
        Pass::Only,
        None,
        None,
        Some(&layer),
    ));
    let joined = args.join(" ");
    assert!(
        joined.contains("-f rawvideo -pix_fmt rgba -s 422x692 -framerate 30.000000 -i pipe:0"),
        "{joined}"
    );
    let graph = &args[args.iter().position(|arg| arg == "-filter_complex").unwrap() + 1];
    assert_eq!(
        graph,
        "[0:v]trim=start=600:end=660[base];\
         [1:v]setpts=PTS-STARTPTS+600/TB,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[chat];\
         [base][chat]overlay=x=1468:y=324:format=yuv420:eof_action=repeat,trim=end=660.000000[v]"
    );
    assert!(joined.contains("-map [v] -map 0:a:0?"), "{joined}");
    assert!(!args.contains(&"-vf".to_string()), "the cut is in the graph");
    assert!(
        joined.contains("-c:v h264_nvenc -pix_fmt yuv420p -preset p1"),
        "{joined}"
    );
    // Never more than half again Twitch's 7777 kbit/s.
    assert!(joined.contains("-maxrate 11665k -bufsize 23330k"), "{joined}");
}

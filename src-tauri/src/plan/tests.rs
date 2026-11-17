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
        smooth_editing: false,
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

// ── Edit ────────────────────────────────────────────────────────────────

#[test]
fn edit_makes_constant_frame_rate_h264_with_48khz_audio_and_short_gops() {
    for target in [TargetId::Premiere, TargetId::Resolve, TargetId::Capcut, TargetId::Vegas] {
        let recipe = plan_with(&youtube_4k(), preset(target), &nvidia());
        let encode = recipe.video_encode().expect("editing always encodes");
        assert_eq!(encode.codec, VideoCodec::H264);
        assert!(encode.constant_frame_rate);
        assert_eq!(encode.keyframe_seconds, Some(1.0));
        assert_eq!(recipe.container, Container::Mp4);
        assert!(matches!(
            recipe.audio.as_ref().map(|a| &a.action),
            Some(AudioAction::Encode {
                codec: AudioCodec::Aac,
                sample_rate: Some(48_000),
                ..
            })
        ));
        assert_eq!(recipe.output_lines(), Some(2160));
    }
    let recipe = plan_with(&youtube_4k(), preset(TargetId::Premiere), &nvidia());
    let args = strings(ffmpeg::convert_args(
        &recipe,
        Path::new("in.mkv"),
        Path::new("out.mp4"),
        Pass::Only,
    ));
    assert!(args.windows(2).any(|pair| pair == ["-g", "30"]));
    assert!(args.windows(2).any(|pair| pair == ["-ar", "48000"]));
    assert!(args.windows(2).any(|pair| pair == ["-fps_mode", "cfr"]));
}

#[test]
fn smoothest_editing_makes_prores_at_1080p_except_for_capcut() {
    let smooth = |target| {
        plan_with(
            &youtube_4k(),
            PresetChoice {
                smooth_editing: true,
                ..preset(target)
            },
            &Capabilities::BASIC,
        )
    };
    let resolve = smooth(TargetId::Resolve);
    assert_eq!(encoded_to(&resolve), Some(VideoCodec::Prores));
    assert_eq!(resolve.container, Container::Mov);
    assert_eq!(resolve.audio_codec(), Some(AudioCodec::Pcm));
    assert_eq!(resolve.output_lines(), Some(1080));
    // ProRes files are big, and the estimate says so.
    assert!(resolve.estimated_size().unwrap() > 5_000_000_000);

    assert_eq!(encoded_to(&smooth(TargetId::Capcut)), Some(VideoCodec::H264));
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
    // The 720p file is 5.4 MB.
    let recipe = share(&x_clip(), TargetId::Discord, TierId::DiscordFree, None);
    assert!(copied(&recipe));
    assert_eq!(recipe.format_selector(), "http-2176");
    assert_eq!(recipe.size_limit, Some(10_000_000));
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
    let trim = Some(Trim {
        start: 60.0,
        end: 90.0,
        exact: true,
    });
    let recipe = share(&youtube_4k(), TargetId::Discord, TierId::DiscordFree, trim);
    assert_eq!(recipe.duration, Some(30.0));
    // 30 seconds into 10 MB leaves room for 720p.
    assert_eq!(recipe.output_lines(), Some(720));
    assert!(recipe.estimated_size().unwrap() <= 10_000_000);
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
fn an_exact_cut_re_encodes_in_the_same_codec_and_counts_from_the_start() {
    let trim = Some(Trim {
        start: 10.0,
        end: 20.0,
        exact: true,
    });
    let recipe = plan(&x_4k(), &Choice::Preset(preset(TargetId::Apple)), trim, &nvidia()).unwrap();
    assert_eq!(encoded_to(&recipe), Some(VideoCodec::H264));
    let yt_dlp = args::recipe_args(&recipe);
    assert!(yt_dlp.iter().any(|arg| arg.contains("-copyts -start_at_zero")));
}

#[test]
fn a_fast_cut_copies_and_scales_the_size_estimate() {
    let trim = Some(Trim {
        start: 0.0,
        end: 60.0,
        exact: false,
    });
    let whole = plan_for(&x_4k(), TargetId::Windows).estimated_size().unwrap();
    let part = plan(
        &x_4k(),
        &Choice::Preset(preset(TargetId::Windows)),
        trim,
        &Capabilities::BASIC,
    )
    .unwrap();
    assert!(copied(&part));
    assert_eq!(part.estimated_size().unwrap(), whole / 10);
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
    let empty = Some(Trim {
        start: 5.0,
        end: 5.0,
        exact: false,
    });
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

use super::*;
use crate::ytdlp::metadata::{Format, FormatKind, VideoInfo};

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
        note: None,
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
        abr: Some(96.0),
        ..video(id, "avc1.42001E", height, 30.0, kbps)
    }
}

fn info(formats: Vec<Format>) -> VideoInfo {
    VideoInfo {
        id: "abc".into(),
        title: "Test".into(),
        url: "https://example.com/watch?v=abc".into(),
        thumbnail: None,
        duration: Some(600.0),
        channel: None,
        view_count: None,
        formats,
    }
}

/// What YouTube typically offers for a 4K upload: H.264 up to 1080p, VP9 and AV1 above.
fn youtube_4k() -> VideoInfo {
    info(vec![
        muxed("18", 360, 500.0),
        video("136", "avc1.4d401f", 720, 30.0, 1500.0),
        video("137", "avc1.640028", 1080, 30.0, 4000.0),
        video("248", "vp09.00.40.08", 1080, 30.0, 2500.0),
        video("313", "vp09.00.50.08", 2160, 30.0, 18000.0),
        video("401", "av01.0.12M.08", 2160, 30.0, 12000.0),
        audio("140", "mp4a.40.2", 129.0),
        audio("251", "opus", 160.0),
        audio("251-drc", "opus", 165.0),
    ])
}

fn preset(preset: PresetId) -> DownloadOptions {
    DownloadOptions::Preset {
        preset,
        max_lines: None,
        size_limit_mb: None,
    }
}

fn preset_at(preset: PresetId, lines: u32) -> DownloadOptions {
    DownloadOptions::Preset {
        preset,
        max_lines: Some(lines),
        size_limit_mb: None,
    }
}

fn manual(content: Content) -> ManualOptions {
    ManualOptions {
        content,
        video_format: None,
        audio_format: None,
        container: None,
        video_codec: None,
        audio_codec: None,
    }
}

#[test]
fn iphone_copies_the_best_native_streams() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Iphone)).unwrap();
    assert_eq!(recipe.format_selector(), "137+140");
    assert_eq!(recipe.container, Container::Mp4);
    assert!(!recipe.needs_ffmpeg_pass());
    // …and mentions that 4K exists but would need converting.
    assert!(recipe.hints.iter().any(|hint| hint.text.contains("2160p")));
    let four_k = recipe.qualities.iter().find(|quality| quality.lines == 2160).unwrap();
    assert!(four_k.converts);
    assert!(
        !recipe
            .qualities
            .iter()
            .find(|quality| quality.lines == 1080)
            .unwrap()
            .converts
    );
}

#[test]
fn iphone_converts_4k_to_hevc_only_when_asked() {
    let recipe = plan(&youtube_4k(), &preset_at(PresetId::Iphone, 2160)).unwrap();
    let step = recipe.video.as_ref().unwrap();
    assert_eq!(step.lines, 2160);
    assert!(matches!(&step.action, VideoAction::Encode(encode) if encode.codec == VideoCodec::Hevc));
    // AAC is copied even though the Opus track has a higher bitrate.
    assert_eq!(recipe.audio.as_ref().unwrap().format_id.as_deref(), Some("140"));
    assert!(!recipe.encodes_audio());
    assert!(recipe.needs_ffmpeg_pass());
}

#[test]
fn android_plays_vp9_so_4k_needs_no_conversion() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Android)).unwrap();
    assert_eq!(recipe.video.as_ref().unwrap().format_id, "313");
    assert!(!recipe.needs_ffmpeg_pass());
    assert_eq!(recipe.container, Container::Mp4);
}

#[test]
fn windows_puts_vp9_in_webm_with_opus() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Windows)).unwrap();
    // H.264 tops out at 1080p; 4K is VP9 (preferred over AV1 on Windows).
    assert_eq!(recipe.format_selector(), "313+251");
    assert_eq!(recipe.container, Container::Webm);
    assert!(!recipe.needs_ffmpeg_pass());
    assert!(recipe.hints.iter().any(|hint| hint.text.contains("VP9")));

    let hd = plan(&youtube_4k(), &preset_at(PresetId::Windows, 1080)).unwrap();
    assert_eq!(hd.format_selector(), "137+140");
    assert_eq!(hd.container, Container::Mp4);
}

#[test]
fn max_quality_takes_the_most_efficient_codec_and_skips_drc_audio() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Max)).unwrap();
    assert_eq!(recipe.format_selector(), "401+251");
    assert!(!recipe.needs_ffmpeg_pass());
    // AV1 fits MP4, but Opus is at home in WebM.
    assert_eq!(recipe.container, Container::Webm);
}

#[test]
fn audio_preset_keeps_the_original_codec() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Audio)).unwrap();
    assert_eq!(recipe.format_selector(), "251");
    assert_eq!(recipe.container, Container::Opus);
    assert!(!recipe.encodes_audio());
}

#[test]
fn editing_makes_constant_frame_rate_dnxhr_at_1080p_by_default() {
    let recipe = plan(&youtube_4k(), &preset(PresetId::Editing)).unwrap();
    let step = recipe.video.as_ref().unwrap();
    assert_eq!(step.lines, 1080);
    assert!(matches!(&step.action, VideoAction::Encode(e) if e.codec == VideoCodec::Dnxhr && e.constant_frame_rate));
    assert!(matches!(
        recipe.audio.as_ref().unwrap().action,
        AudioAction::Encode {
            codec: AudioCodec::Pcm,
            ..
        }
    ));
    assert_eq!(recipe.container, Container::Mov);
    // ~220 Mbit/s for ten minutes is about 16.5 GB.
    let size = recipe.estimated_size().unwrap();
    assert!((16_000_000_000..18_000_000_000).contains(&size), "{size}");
}

#[test]
fn share_copies_a_stream_that_already_fits() {
    // 720p (112 MB) + AAC (9.7 MB) fits under 500 MB; so does 1080p (300 MB).
    let options = DownloadOptions::Preset {
        preset: PresetId::Share,
        max_lines: None,
        size_limit_mb: Some(500),
    };
    let recipe = plan(&youtube_4k(), &options).unwrap();
    assert_eq!(recipe.format_selector(), "137+140");
    assert!(!recipe.needs_ffmpeg_pass());
}

#[test]
fn share_converts_to_a_bitrate_that_fits() {
    let options = DownloadOptions::Preset {
        preset: PresetId::Share,
        max_lines: None,
        size_limit_mb: Some(10),
    };
    let recipe = plan(&youtube_4k(), &options).unwrap();
    let step = recipe.video.as_ref().unwrap();
    let VideoAction::Encode(encode) = &step.action else {
        panic!("should convert")
    };
    assert_eq!(encode.codec, VideoCodec::H264);
    // 10 MB over ten minutes is ~128 kbit/s in total: tiny, and flagged as such.
    assert!(encode.bitrate_kbps.unwrap() < 100);
    assert!(recipe.hints.iter().any(|hint| hint.level == HintLevel::Warning));
    assert!(recipe.estimated_size().unwrap() <= 10_000_000);
    // It downloads the smallest source that covers the target, not 4K.
    assert_eq!(step.lines, 360);
}

#[test]
fn a_site_with_one_combined_file_downloads_just_that() {
    let recipe = plan(&info(vec![muxed("mp4-720", 720, 2000.0)]), &preset(PresetId::Iphone)).unwrap();
    assert_eq!(recipe.format_selector(), "mp4-720");
    assert!(!recipe.needs_ffmpeg_pass());
}

#[test]
fn audio_only_sources_ignore_video_presets() {
    let recipe = plan(&info(vec![audio("mp3", "mp3", 128.0)]), &preset(PresetId::Iphone)).unwrap();
    assert!(recipe.video.is_none());
    assert_eq!(recipe.container, Container::Mp3);
}

#[test]
fn manual_lossless_conversion_warns_that_it_only_adds_size() {
    let options = ManualOptions {
        audio_codec: Some(AudioCodec::Flac),
        ..manual(Content::AudioOnly)
    };
    let recipe = plan(&youtube_4k(), &DownloadOptions::Manual(options)).unwrap();
    assert_eq!(recipe.container, Container::Flac);
    assert!(recipe.encodes_audio());
    let hint = recipe
        .hints
        .iter()
        .find(|hint| hint.level == HintLevel::Warning)
        .unwrap();
    assert!(hint.text.contains("160 kbps Opus"), "{}", hint.text);
}

#[test]
fn manual_container_forces_compatible_codecs() {
    // MOV can't hold VP9 or Opus.
    let options = ManualOptions {
        video_format: Some("313".into()),
        audio_format: Some("251".into()),
        container: Some(Container::Mov),
        ..manual(Content::VideoAudio)
    };
    let recipe = plan(&youtube_4k(), &DownloadOptions::Manual(options)).unwrap();
    assert_eq!(recipe.video_codec(), Some(VideoCodec::H264));
    assert_eq!(recipe.audio_codec(), Some(AudioCodec::Aac));
    assert!(recipe.needs_ffmpeg_pass());
}

#[test]
fn manual_defaults_copy_everything() {
    let recipe = plan(&youtube_4k(), &DownloadOptions::Manual(manual(Content::VideoAudio))).unwrap();
    assert_eq!(recipe.format_selector(), "401+251");
    assert!(!recipe.needs_ffmpeg_pass());
    assert!(recipe.hints.is_empty());
}

#[test]
fn manual_video_only_strips_audio_from_a_combined_stream() {
    let options = ManualOptions {
        video_format: Some("18".into()),
        ..manual(Content::VideoOnly)
    };
    let recipe = plan(&youtube_4k(), &DownloadOptions::Manual(options)).unwrap();
    assert!(recipe.strip_audio);
    assert_eq!(recipe.audio_codec(), None);
    assert!(recipe.needs_ffmpeg_pass());
}

#[test]
fn unknown_stream_ids_are_rejected() {
    let options = ManualOptions {
        video_format: Some("nope".into()),
        ..manual(Content::VideoAudio)
    };
    assert!(plan(&youtube_4k(), &DownloadOptions::Manual(options)).is_err());
}

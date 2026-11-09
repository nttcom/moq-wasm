use std::cmp::Reverse;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, aac::AudioSpecificConfig, loc::VideoCodec};
use msf::{Catalog, KnownPackaging, KnownTrackRole, Packaging, Track};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Selection {
    pub(crate) video: Option<VideoTrack>,
    pub(crate) audio: Option<AudioTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoTrack {
    pub(crate) name: String,
    pub(crate) codec: VideoCodec,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AudioTrack {
    pub(crate) name: String,
    pub(crate) config: AudioConfig,
}

#[derive(Clone, Copy)]
pub(crate) struct Choice<'a> {
    pub(crate) source: SourceChoice,
    pub(crate) video: VideoChoice<'a>,
    pub(crate) audio: AudioChoice<'a>,
}

/// Which source of a broadcast a recording reads. A source is the LOC video
/// and audio tracks of one `renderGroup` (draft-ietf-moq-msf-01 §5.2.11), such
/// as a camera with its renditions and microphone, or a shared screen; the
/// tracks without a `renderGroup` form one source. `broadcast` renumbers the
/// `renderGroup`s in each catalog, so the source is found again in every
/// catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceChoice {
    /// The source `broadcast` and the browser SDK publish by default, such as
    /// a meeting participant's camera and microphone: the one listing a media
    /// track named `video`, `audio` or `video_<rendition>`. None while no such
    /// track is listed, unless the catalog has no event timeline, as a
    /// broadcast outside a meeting such as `onvif-ingest`'s: then its first
    /// source.
    Main,
    /// The first source listed besides the main one, such as a meeting
    /// participant's screen share.
    Other,
}

#[derive(Clone, Copy)]
pub(crate) enum VideoChoice<'a> {
    /// Whichever source lists it.
    Named(&'a str),
    /// The source's tallest video track at most this many pixels high, or the
    /// shortest one when all are taller; ties go to the lower `bitrate`. A
    /// `broadcast` source's video tracks are the members of its `altGroup`
    /// (§5.2.12), or its one video.
    MaxHeight(u32),
}

#[derive(Clone, Copy)]
pub(crate) enum AudioChoice<'a> {
    /// Whichever source lists it.
    Named(&'a str),
    /// The source's decodable audio track with the highest `bitrate`.
    HighestBitrate,
}

/// A source, named by its tracks' `renderGroup`; `None` names the tracks
/// without one.
type Source = Option<u64>;

/// `Ok(None)` while the chosen source lists neither a video track nor an
/// audio track that can be decoded; a meeting participant with the camera off lists
/// only the audio. `Err` when the named video track, or every video track of
/// the source, has a codec other than H.264, VP8, VP9 or AV1.
///
/// Without a name, only LOC tracks with role `video` or `audio` qualify.
/// Remaining ties go to the track listed first.
///
/// The audio is `None` while no such track can be decoded from the catalog:
/// `broadcast` lists AAC audio before the first audio frame delivers its
/// configuration.
pub(crate) fn select_tracks(catalog: &Catalog, choice: Choice) -> Result<Option<Selection>> {
    let video = select_video(catalog, choice)?;
    let audio = select_audio(catalog, choice)?;
    Ok((video.is_some() || audio.is_some()).then_some(Selection { video, audio }))
}

/// The video track `select_tracks` picks.
pub(crate) fn select_video(catalog: &Catalog, choice: Choice) -> Result<Option<VideoTrack>> {
    let max_height = match choice.video {
        VideoChoice::Named(name) => {
            return match catalog.loc_tracks().find(|track| track.name == name) {
                Some(track) => video_track(track).map(Some),
                None => Ok(None),
            };
        }
        VideoChoice::MaxHeight(max_height) => max_height,
    };
    let listed: Vec<&Track> = source_tracks(catalog, choice.source)
        .filter(|track| track.has_role(KnownTrackRole::Video))
        .collect();
    if listed.is_empty() {
        return Ok(None);
    }
    // `max_by_key` keeps the last maximum, so the reversed list keeps the first.
    listed
        .iter()
        .rev()
        .filter_map(|track| Some((*track, video_track(track).ok()?)))
        .max_by_key(|(track, _)| rendition_rank(track, max_height))
        .map(|(_, video)| Some(video))
        .with_context(|| {
            let codecs: Vec<String> = listed
                .iter()
                .map(|track| format!("{} ({})", track.name, track.codec.as_deref().unwrap_or("-")))
                .collect();
            format!(
                "no video track in the catalog can be recorded: {}",
                codecs.join(", ")
            )
        })
}

/// A missing field ranks last.
fn rendition_rank(track: &Track, max_height: u32) -> (bool, i64, Reverse<u64>) {
    let height = i64::from(track.height.unwrap_or(u32::MAX));
    let fits = height <= i64::from(max_height);
    let size = if fits { height } else { -height };
    (fits, size, Reverse(track.bitrate.unwrap_or(u64::MAX)))
}

fn video_track(track: &Track) -> Result<VideoTrack> {
    let codec = track.codec.as_deref().unwrap_or("-");
    Ok(VideoTrack {
        name: track.name.clone(),
        codec: VideoCodec::from_codec_string(codec).with_context(|| {
            format!(
                "video track {} has codec {codec}, which cannot be recorded",
                track.name
            )
        })?,
    })
}

/// The audio track `select_tracks` picks.
pub(crate) fn select_audio(catalog: &Catalog, choice: Choice) -> Result<Option<AudioTrack>> {
    let name = match choice.audio {
        AudioChoice::Named(name) => name,
        AudioChoice::HighestBitrate => {
            let mut decodable = Vec::new();
            for track in source_tracks(catalog, choice.source)
                .filter(|track| track.has_role(KnownTrackRole::Audio))
            {
                if let Some(config) = audio_config(catalog, track)? {
                    let audio = AudioTrack {
                        name: track.name.clone(),
                        config,
                    };
                    decodable.push((track.bitrate, audio));
                }
            }
            return Ok(decodable
                .into_iter()
                .rev()
                .max_by_key(|(bitrate, _)| *bitrate)
                .map(|(_, audio)| audio));
        }
    };
    let Some(track) = catalog.loc_tracks().find(|track| track.name == name) else {
        return Ok(None);
    };
    Ok(audio_config(catalog, track)?.map(|config| AudioTrack {
        name: track.name.clone(),
        config,
    }))
}

/// Whether a recording that reads `track` goes on with it after `catalog`:
/// while the catalog lists it, and unless it was named, in a source of the
/// kind the choice takes.
pub(crate) fn keeps(catalog: &Catalog, choice: Choice, track: &str) -> bool {
    let Some(listed) = catalog.loc_tracks().find(|listed| listed.name == track) else {
        return false;
    };
    let named = matches!(choice.video, VideoChoice::Named(name) if name == track)
        || matches!(choice.audio, AudioChoice::Named(name) if name == track);
    let in_main = main_source(catalog) == Some(listed.render_group);
    named || in_main == (choice.source == SourceChoice::Main)
}

/// The event timeline tracks (draft-ietf-moq-msf-01 §8), such as a meeting's
/// chat.
pub(crate) fn event_timelines(catalog: &Catalog) -> impl Iterator<Item = &Track> {
    packaged(catalog, KnownPackaging::EventTimeline)
}

/// The media timeline tracks (draft-ietf-moq-msf-01 §7) whose `depends` names
/// any of `tracks`.
pub(crate) fn media_timelines<'a>(
    catalog: &'a Catalog,
    tracks: &[&str],
) -> impl Iterator<Item = &'a Track> {
    packaged(catalog, KnownPackaging::MediaTimeline).filter(|timeline| {
        timeline
            .depends
            .iter()
            .flatten()
            .any(|name| tracks.contains(&name.as_str()))
    })
}

fn packaged(catalog: &Catalog, packaging: KnownPackaging) -> impl Iterator<Item = &Track> {
    catalog.tracks.iter().flatten().filter(
        move |track| matches!(&track.packaging, Packaging::Known(known) if *known == packaging),
    )
}

/// The LOC video and audio tracks of the source `choice` takes.
fn source_tracks(catalog: &Catalog, choice: SourceChoice) -> impl Iterator<Item = &Track> {
    let main = main_source(catalog);
    let source = match choice {
        SourceChoice::Main => main,
        SourceChoice::Other => sources(catalog)
            .into_iter()
            .find(|source| Some(*source) != main),
    };
    media_tracks(catalog).filter(move |track| Some(track.render_group) == source)
}

/// `None` while no source is the main one (see `SourceChoice::Main`).
fn main_source(catalog: &Catalog) -> Option<Source> {
    let default = media_tracks(catalog)
        .find(|track| {
            matches!(track.name.as_str(), "video" | "audio") || track.name.starts_with("video_")
        })
        .map(|track| track.render_group);
    if default.is_none() && event_timelines(catalog).next().is_none() {
        return sources(catalog).first().copied();
    }
    default
}

/// In the order the catalog lists them.
fn sources(catalog: &Catalog) -> Vec<Source> {
    let mut sources = Vec::new();
    for track in media_tracks(catalog) {
        if !sources.contains(&track.render_group) {
            sources.push(track.render_group);
        }
    }
    sources
}

fn media_tracks(catalog: &Catalog) -> impl Iterator<Item = &Track> {
    catalog.loc_tracks().filter(|track| {
        track.has_role(KnownTrackRole::Video) || track.has_role(KnownTrackRole::Audio)
    })
}

/// `Ok(None)` for an AAC track without a resolvable `initRef` and for codecs
/// other than AAC, Opus, G.711 A-law and MP3; `Err` for an entry that is
/// present but broken.
fn audio_config(catalog: &Catalog, track: &Track) -> Result<Option<AudioConfig>> {
    let codec = track.codec.as_deref().unwrap_or_default();
    if codec.starts_with("mp4a.40.") {
        let Some(data) = catalog.init_data(track) else {
            return Ok(None);
        };
        let bytes = general_purpose::STANDARD
            .decode(data)
            .context("audio initDataList entry is not Base64")?;
        return Ok(Some(AudioConfig::Aac(AudioSpecificConfig::parse(&bytes)?)));
    }
    if !matches!(codec, "opus" | "pcma" | "mp3") {
        return Ok(None);
    }
    let sample_rate = track
        .sample_rate
        .with_context(|| format!("{codec} audio track lists no sampleRate"))?;
    let channels = channel_count(track.channel_config.as_deref().unwrap_or("mono"))?;
    Ok(Some(match codec {
        "opus" => AudioConfig::Opus {
            sample_rate,
            channels,
        },
        "pcma" => AudioConfig::Pcma {
            sample_rate,
            channels,
        },
        _ => AudioConfig::Mp3 {
            sample_rate,
            channels,
        },
    }))
}

/// The labels `broadcast` writes: `mono`, `stereo` or `<n>ch`.
fn channel_count(label: &str) -> Result<u8> {
    match label {
        "mono" => Ok(1),
        "stereo" => Ok(2),
        other => other
            .strip_suffix("ch")
            .and_then(|count| count.parse().ok())
            .with_context(|| format!("unknown channelConfig {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aac_config() -> AudioSpecificConfig {
        AudioSpecificConfig::new(2, 48_000, 2)
    }

    fn video_track() -> Track {
        let mut track = Track::new("video", KnownPackaging::Loc, true);
        track.role = Some(KnownTrackRole::Video.into());
        track.codec = Some("avc1.42C01E".to_string());
        track
    }

    fn audio_track(codec: &str) -> Track {
        let mut track = Track::new("audio", KnownPackaging::Loc, true);
        track.role = Some(KnownTrackRole::Audio.into());
        track.codec = Some(codec.to_string());
        track.sample_rate = Some(48_000);
        track.channel_config = Some("stereo".to_string());
        track
    }

    fn catalog(tracks: Vec<Track>) -> Catalog {
        Catalog::new(tracks, 0)
    }

    fn sized_video(name: &str, codec: &str, height: u32, bitrate: u64) -> Track {
        let mut track = video_track();
        track.name = name.to_string();
        track.codec = Some(codec.to_string());
        track.width = Some(height * 16 / 9);
        track.height = Some(height);
        track.bitrate = Some(bitrate);
        track
    }

    fn opus_track(name: &str, bitrate: u64) -> Track {
        let mut track = audio_track("opus");
        track.name = name.to_string();
        track.bitrate = Some(bitrate);
        track
    }

    fn in_source(mut track: Track, render_group: u64) -> Track {
        track.render_group = Some(render_group);
        track
    }

    fn chat() -> Track {
        Track::new("chat", KnownPackaging::EventTimeline, true)
    }

    fn choice(source: SourceChoice, max_height: u32) -> Choice<'static> {
        Choice {
            source,
            video: VideoChoice::MaxHeight(max_height),
            audio: AudioChoice::HighestBitrate,
        }
    }

    fn named<'a>(video: &'a str, audio: &'a str) -> Choice<'a> {
        Choice {
            source: SourceChoice::Main,
            video: VideoChoice::Named(video),
            audio: AudioChoice::Named(audio),
        }
    }

    /// A meeting participant's catalog as `broadcast` numbers it: the screen
    /// share's `renderGroup` sorts first.
    fn meeting() -> Catalog {
        catalog(vec![
            in_source(
                sized_video("share_1080p", "av01.0.08M.08", 1080, 2_000_000),
                1,
            ),
            in_source(
                sized_video("share_720p", "av01.0.08M.08", 720, 1_200_000),
                1,
            ),
            in_source(opus_track("share_audio", 64_000), 1),
            in_source(sized_video("video", "avc1.640028", 1080, 1_000_000), 2),
            in_source(sized_video("video_720p", "avc1.42001F", 720, 500_000), 2),
            in_source(sized_video("video_480p", "avc1.42001F", 480, 200_000), 2),
            in_source(
                sized_video("video_hevc", "hvc1.1.6.L93.B0", 480, 100_000),
                2,
            ),
            in_source(opus_track("audio", 32_000), 2),
            in_source(opus_track("audio_128kbps", 128_000), 2),
            chat(),
        ])
    }

    #[test]
    fn the_main_source_is_the_one_with_the_default_track_names() {
        // Arrange
        let catalog = meeting();

        // Act
        let main = select_tracks(&catalog, choice(SourceChoice::Main, 720))
            .unwrap()
            .unwrap();
        let other = select_tracks(&catalog, choice(SourceChoice::Other, 720))
            .unwrap()
            .unwrap();

        // Assert: each takes its own source's tallest video at most 720 high and its highest bitrate audio
        assert_eq!(main.video.unwrap().name, "video_720p");
        assert_eq!(main.audio.unwrap().name, "audio_128kbps");
        assert_eq!(other.video.unwrap().name, "share_720p");
        assert_eq!(other.audio.unwrap().name, "share_audio");
    }

    #[test]
    fn the_media_timelines_of_a_source_are_those_that_depend_on_its_tracks() {
        // Arrange
        let mut catalog = meeting();
        let timeline = |name: &str, depends: &[&str]| {
            let mut timeline = Track::new(name, KnownPackaging::MediaTimeline, true);
            timeline.depends = Some(depends.iter().map(|name| name.to_string()).collect());
            timeline
        };
        catalog.tracks.as_mut().unwrap().extend([
            timeline(
                "share_1080p_timeline",
                &["share_1080p", "share_720p", "share_audio"],
            ),
            timeline("video_timeline", &["video", "video_720p", "video_480p"]),
            timeline("audio_timeline", &["audio", "audio_128kbps"]),
        ]);

        // Act
        let names: Vec<&str> = media_timelines(&catalog, &["video_720p", "audio_128kbps"])
            .map(|track| track.name.as_str())
            .collect();

        // Assert
        assert_eq!(names, ["video_timeline", "audio_timeline"]);
    }

    #[test]
    fn picks_the_tallest_recordable_video_under_the_height_or_else_the_shortest() {
        // Arrange
        let catalog = meeting();

        // Act
        let under = select_video(&catalog, choice(SourceChoice::Main, 480)).unwrap();
        let over = select_video(&catalog, choice(SourceChoice::Main, 240)).unwrap();

        // Assert: video_hevc cannot be recorded, so video_480p wins both ties at 480 pixels
        assert_eq!(under.unwrap().name, "video_480p");
        assert_eq!(over.unwrap().name, "video_480p");
    }

    #[test]
    fn a_catalog_without_the_default_tracks_has_no_main_source() {
        // Arrange: a participant who shares the screen with the camera and the microphone off
        let catalog = catalog(vec![
            in_source(
                sized_video("share_720p", "av01.0.08M.08", 720, 1_200_000),
                1,
            ),
            chat(),
        ]);

        // Act
        let main = select_tracks(&catalog, choice(SourceChoice::Main, 720)).unwrap();
        let other = select_tracks(&catalog, choice(SourceChoice::Other, 720))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(main, None);
        assert_eq!(other.video.unwrap().name, "share_720p");
        assert_eq!(other.audio, None);
    }

    #[test]
    fn a_camera_rendition_listed_before_the_camera_is_the_main_source() {
        // Arrange: the microphone off, so only `video_480p` marks the camera
        let catalog = catalog(vec![
            in_source(
                sized_video("share_720p", "av01.0.08M.08", 720, 1_200_000),
                1,
            ),
            in_source(sized_video("video_480p", "avc1.42001F", 480, 200_000), 2),
        ]);

        // Act
        let main = select_video(&catalog, choice(SourceChoice::Main, 720)).unwrap();
        let other = select_video(&catalog, choice(SourceChoice::Other, 720)).unwrap();

        // Assert
        assert_eq!(main.unwrap().name, "video_480p");
        assert_eq!(other.unwrap().name, "share_720p");
    }

    #[test]
    fn without_an_event_timeline_the_first_source_is_the_main_one() {
        // Arrange: a broadcast outside a meeting, its tracks named as `onvif-ingest` names them
        let catalog = catalog(vec![
            in_source(
                sized_video("video/profile_1", "avc1.42001F", 480, 200_000),
                7,
            ),
            in_source(
                sized_video("video/profile_2", "avc1.42001F", 480, 200_000),
                3,
            ),
        ]);

        // Act
        let main = select_video(&catalog, choice(SourceChoice::Main, 720)).unwrap();
        let other = select_video(&catalog, choice(SourceChoice::Other, 720)).unwrap();

        // Assert
        assert_eq!(main.unwrap().name, "video/profile_1");
        assert_eq!(other.unwrap().name, "video/profile_2");
    }

    #[test]
    fn tracks_without_a_render_group_form_one_source() {
        // Arrange
        let catalog = catalog(vec![
            sized_video("video_480p", "avc1.42001F", 480, 200_000),
            sized_video("video_720p", "avc1.42001F", 720, 500_000),
            opus_track("audio", 64_000),
        ]);

        // Act
        let main = select_tracks(&catalog, choice(SourceChoice::Main, 720))
            .unwrap()
            .unwrap();
        let other = select_tracks(&catalog, choice(SourceChoice::Other, 720)).unwrap();

        // Assert
        assert_eq!(main.video.unwrap().name, "video_720p");
        assert_eq!(main.audio.unwrap().name, "audio");
        assert_eq!(other, None);
    }

    #[test]
    fn keeps_a_track_while_its_source_is_of_the_kind_the_choice_takes() {
        // Arrange: a meeting source listed alone, then with `video`, which makes it the camera
        let alone = catalog(vec![
            in_source(sized_video("extra_480p", "avc1.42001F", 480, 200_000), 1),
            chat(),
        ]);
        let joined = catalog(vec![
            in_source(sized_video("video", "avc1.42001F", 1080, 1_000_000), 1),
            in_source(sized_video("extra_480p", "avc1.42001F", 480, 200_000), 1),
            in_source(
                sized_video("share_720p", "av01.0.08M.08", 720, 1_200_000),
                2,
            ),
            in_source(
                sized_video("extra_720p", "av01.0.08M.08", 720, 1_200_000),
                3,
            ),
            chat(),
        ]);
        let other = choice(SourceChoice::Other, 720);

        // Act / Assert
        assert!(keeps(&alone, other, "extra_480p"));
        assert!(!keeps(&joined, other, "extra_480p"));
        assert!(keeps(
            &joined,
            choice(SourceChoice::Main, 720),
            "extra_480p"
        ));
        assert!(keeps(&joined, other, "extra_720p"));
        assert!(!keeps(&joined, other, "gone"));
        assert!(keeps(&joined, named("share_720p", "audio"), "share_720p"));
    }

    #[test]
    fn a_named_track_wins_over_the_source() {
        // Arrange
        let catalog = meeting();

        // Act
        let selection = select_tracks(&catalog, named("share_1080p", "audio"))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(selection.video.unwrap().name, "share_1080p");
        assert_eq!(selection.audio.unwrap().name, "audio");
    }

    #[test]
    fn fails_when_no_video_track_of_the_source_can_be_recorded() {
        // Arrange
        let catalog = catalog(vec![sized_video("video", "hvc1.1.6.L93.B0", 1080, 0)]);

        // Act
        let error = select_tracks(&catalog, choice(SourceChoice::Main, 720)).unwrap_err();

        // Assert
        assert_eq!(
            error.to_string(),
            "no video track in the catalog can be recorded: video (hvc1.1.6.L93.B0)"
        );
    }

    #[test]
    fn resolves_the_aac_configuration_from_the_init_data_entry() {
        // Arrange
        let mut audio = audio_track("mp4a.40.2");
        let init_data =
            audio.set_inline_init_data(general_purpose::STANDARD.encode(aac_config().to_bytes()));
        let mut catalog = catalog(vec![video_track(), audio]);
        catalog.init_data_list = Some(vec![init_data]);

        // Act
        let selection = select_tracks(&catalog, named("video", "audio")).unwrap();

        // Assert
        assert_eq!(
            selection,
            Some(Selection {
                video: Some(VideoTrack {
                    name: "video".to_string(),
                    codec: VideoCodec::H264,
                }),
                audio: Some(AudioTrack {
                    name: "audio".to_string(),
                    config: AudioConfig::Aac(aac_config()),
                }),
            })
        );
    }

    #[test]
    fn waits_while_neither_track_is_listed() {
        // Arrange: the AAC track has no init data yet
        let catalog = catalog(vec![audio_track("mp4a.40.2")]);

        // Act / Assert
        assert_eq!(
            select_tracks(&catalog, named("video", "audio")).unwrap(),
            None
        );
        assert_eq!(
            select_tracks(&catalog, choice(SourceChoice::Main, 720)).unwrap(),
            None
        );
    }

    #[test]
    fn records_audio_only_while_no_video_track_is_listed() {
        // Arrange: a meeting participant with the camera off
        let catalog = catalog(vec![in_source(audio_track("opus"), 1)]);

        // Act
        let selection = select_tracks(&catalog, choice(SourceChoice::Main, 720))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(selection.video, None);
        assert_eq!(selection.audio.unwrap().name, "audio");
    }

    #[test]
    fn records_video_only_when_the_aac_track_has_no_init_data_yet() {
        // Arrange
        let catalog = catalog(vec![video_track(), audio_track("mp4a.40.2")]);

        // Act
        let selection = select_tracks(&catalog, named("video", "audio"))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(selection.audio, None);
    }

    #[test]
    fn reads_opus_alaw_and_mp3_from_the_sample_rate_and_channel_config() {
        // Arrange
        let mut alaw = audio_track("pcma");
        alaw.name = "audio/profile_1".to_string();
        alaw.sample_rate = Some(8_000);
        alaw.channel_config = Some("mono".to_string());
        let mut mp3 = audio_track("mp3");
        mp3.name = "audio_mp3".to_string();
        let catalog = catalog(vec![video_track(), audio_track("opus"), alaw, mp3]);

        // Act
        let opus = select_audio(&catalog, named("video", "audio")).unwrap();
        let pcma = select_audio(&catalog, named("video", "audio/profile_1")).unwrap();
        let mp3 = select_audio(&catalog, named("video", "audio_mp3")).unwrap();

        // Assert
        assert_eq!(
            opus.unwrap().config,
            AudioConfig::Opus {
                sample_rate: 48_000,
                channels: 2
            }
        );
        assert_eq!(
            pcma.unwrap().config,
            AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 1
            }
        );
        assert_eq!(
            mp3.unwrap().config,
            AudioConfig::Mp3 {
                sample_rate: 48_000,
                channels: 2
            }
        );
    }

    #[test]
    fn takes_the_video_codec_from_the_catalog() {
        // Arrange
        let mut av1 = video_track();
        av1.codec = Some("av01.0.08M.08".to_string());
        let mut hevc = video_track();
        hevc.name = "hevc".to_string();
        hevc.codec = Some("hvc1.1.6.L93.B0".to_string());
        let catalog = catalog(vec![av1, hevc]);

        // Act
        let selection = select_video(&catalog, named("video", "audio")).unwrap();
        let error = select_video(&catalog, named("hevc", "audio")).unwrap_err();

        // Assert
        assert_eq!(selection.unwrap().codec, VideoCodec::Av1);
        assert_eq!(
            error.to_string(),
            "video track hevc has codec hvc1.1.6.L93.B0, which cannot be recorded"
        );
    }

    #[test]
    fn only_loc_tracks_qualify() {
        // Arrange
        let mut cmaf = video_track();
        cmaf.packaging = Packaging::Known(KnownPackaging::Cmaf);
        let catalog = catalog(vec![cmaf]);

        // Act / Assert
        assert_eq!(
            select_tracks(&catalog, named("video", "audio")).unwrap(),
            None
        );
        assert_eq!(
            select_tracks(&catalog, choice(SourceChoice::Main, 720)).unwrap(),
            None
        );
    }
}

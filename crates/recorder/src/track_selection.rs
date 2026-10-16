use std::cmp::Reverse;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, aac::AudioSpecificConfig, loc::VideoCodec};
use msf::{Catalog, KnownPackaging, KnownTrackRole, Packaging, Track, TrackRole};

#[derive(Debug, PartialEq)]
pub(crate) struct Selection {
    pub(crate) video: VideoTrack,
    pub(crate) audio: Option<AudioTrack>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct VideoTrack {
    pub(crate) name: String,
    pub(crate) codec: VideoCodec,
}

#[derive(Debug, PartialEq)]
pub(crate) struct AudioTrack {
    pub(crate) name: String,
    pub(crate) config: AudioConfig,
}

#[derive(Clone, Copy)]
pub(crate) enum VideoChoice<'a> {
    Named(&'a str),
    /// The tallest video track at most this many pixels high, or the shortest
    /// one when all are taller; ties go to the lower `bitrate`.
    MaxHeight(u32),
}

/// `Ok(None)` until the catalog lists the video track, which a broadcast does
/// from its first keyframe. `Err` when the named video track, or every listed
/// one, has a codec other than H.264, VP8, VP9 or AV1.
///
/// Without a video name, only LOC tracks with role `video` qualify. Without an
/// audio name, the audio track is the decodable LOC track with role `audio`
/// and the highest `bitrate`. Remaining ties go to the track listed first.
///
/// The audio is `None` while no such track can be decoded from the catalog:
/// `broadcast` lists AAC audio before the first audio frame delivers its
/// configuration.
pub(crate) fn select_tracks(
    catalog: &Catalog,
    video: VideoChoice,
    audio_name: Option<&str>,
) -> Result<Option<Selection>> {
    let Some(video) = select_video(catalog, video)? else {
        return Ok(None);
    };
    let audio = select_audio(catalog, audio_name)?;
    Ok(Some(Selection { video, audio }))
}

fn select_video(catalog: &Catalog, choice: VideoChoice) -> Result<Option<VideoTrack>> {
    let max_height = match choice {
        VideoChoice::Named(name) => {
            return match loc_tracks(catalog).find(|track| track.name == name) {
                Some(track) => video_track(track).map(Some),
                None => Ok(None),
            };
        }
        VideoChoice::MaxHeight(max_height) => max_height,
    };
    let listed: Vec<&Track> = loc_tracks(catalog)
        .filter(|track| has_role(track, KnownTrackRole::Video))
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
pub(crate) fn select_audio(catalog: &Catalog, name: Option<&str>) -> Result<Option<AudioTrack>> {
    let Some(name) = name else {
        let mut decodable = Vec::new();
        for track in loc_tracks(catalog).filter(|track| has_role(track, KnownTrackRole::Audio)) {
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
    };
    let Some(track) = loc_tracks(catalog).find(|track| track.name == name) else {
        return Ok(None);
    };
    Ok(audio_config(catalog, track)?.map(|config| AudioTrack {
        name: track.name.clone(),
        config,
    }))
}

pub(crate) fn loc_tracks(catalog: &Catalog) -> impl Iterator<Item = &Track> {
    catalog
        .tracks
        .iter()
        .flatten()
        .filter(|track| track.packaging == Packaging::Known(KnownPackaging::Loc))
}

fn has_role(track: &Track, role: KnownTrackRole) -> bool {
    track.role == Some(TrackRole::Known(role))
}

/// `Ok(None)` for an AAC track without a resolvable `initRef` and for codecs
/// other than AAC, Opus, G.711 A-law and MP3; `Err` for an entry that is
/// present but broken.
fn audio_config(catalog: &Catalog, track: &Track) -> Result<Option<AudioConfig>> {
    let codec = track.codec.as_deref().unwrap_or_default();
    if codec.starts_with("mp4a.40.") {
        let Some(data) = init_data(catalog, track) else {
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

fn init_data<'a>(catalog: &'a Catalog, track: &Track) -> Option<&'a str> {
    let init_ref = track.init_ref.as_deref()?;
    catalog
        .init_data_list
        .as_deref()?
        .iter()
        .find(|entry| entry.id == init_ref)
        .map(|entry| entry.data.as_str())
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

    /// A meeting participant's catalog.
    fn renditions() -> Catalog {
        catalog(vec![
            sized_video("camera_base_720p", "avc1.42001F", 720, 500_000),
            sized_video("camera_base_480p", "avc1.42001F", 480, 200_000),
            sized_video("camera_main_1080p", "avc1.4D4028", 1080, 1_000_000),
            sized_video("camera_main_480p", "avc1.4D4028", 480, 200_000),
            sized_video("screenshare_480p", "av01.0.08M.08", 480, 700_000),
            sized_video("hevc_480p", "hvc1.1.6.L93.B0", 480, 100_000),
            opus_track("audio_32kbps", 32_000),
            opus_track("audio_128kbps", 128_000),
            opus_track("audio_64kbps", 64_000),
        ])
    }

    #[test]
    fn without_names_picks_the_lightest_recordable_video_at_the_height_and_the_highest_bitrate_audio()
     {
        // Arrange
        let catalog = renditions();

        // Act
        let selection = select_tracks(&catalog, VideoChoice::MaxHeight(480), None)
            .unwrap()
            .unwrap();

        // Assert: hevc_480p cannot be recorded, screenshare_480p has a higher bitrate, the two cameras tie and the first listed wins
        assert_eq!(selection.video.name, "camera_base_480p");
        assert_eq!(selection.audio.unwrap().name, "audio_128kbps");
    }

    #[test]
    fn without_names_picks_the_tallest_video_under_the_height_or_else_the_shortest() {
        // Arrange
        let catalog = renditions();

        // Act
        let under = select_tracks(&catalog, VideoChoice::MaxHeight(900), None)
            .unwrap()
            .unwrap();
        let over = select_tracks(&catalog, VideoChoice::MaxHeight(240), None)
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(under.video.name, "camera_base_720p");
        assert_eq!(over.video.name, "camera_base_480p");
    }

    #[test]
    fn a_named_track_wins_over_the_automatic_choice() {
        // Arrange
        let catalog = renditions();

        // Act
        let selection = select_tracks(
            &catalog,
            VideoChoice::Named("camera_main_1080p"),
            Some("audio_32kbps"),
        )
        .unwrap()
        .unwrap();

        // Assert
        assert_eq!(selection.video.name, "camera_main_1080p");
        assert_eq!(selection.audio.unwrap().name, "audio_32kbps");
    }

    #[test]
    fn without_names_fails_when_no_listed_video_track_can_be_recorded() {
        // Arrange
        let catalog = catalog(vec![sized_video("hevc", "hvc1.1.6.L93.B0", 1080, 0)]);

        // Act
        let error = select_tracks(&catalog, VideoChoice::MaxHeight(480), None).unwrap_err();

        // Assert
        assert_eq!(
            error.to_string(),
            "no video track in the catalog can be recorded: hevc (hvc1.1.6.L93.B0)"
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
        let selection =
            select_tracks(&catalog, VideoChoice::Named("video"), Some("audio")).unwrap();

        // Assert
        assert_eq!(
            selection,
            Some(Selection {
                video: VideoTrack {
                    name: "video".to_string(),
                    codec: VideoCodec::H264,
                },
                audio: Some(AudioTrack {
                    name: "audio".to_string(),
                    config: AudioConfig::Aac(aac_config()),
                }),
            })
        );
    }

    #[test]
    fn waits_while_the_video_track_is_not_listed() {
        // Arrange
        let catalog = catalog(vec![audio_track("opus")]);

        // Act / Assert
        assert_eq!(
            select_tracks(&catalog, VideoChoice::Named("video"), Some("audio")).unwrap(),
            None
        );
        assert_eq!(
            select_tracks(&catalog, VideoChoice::MaxHeight(480), None).unwrap(),
            None
        );
    }

    #[test]
    fn records_video_only_when_the_aac_track_has_no_init_data_yet() {
        // Arrange
        let catalog = catalog(vec![video_track(), audio_track("mp4a.40.2")]);

        // Act
        let selection = select_tracks(&catalog, VideoChoice::Named("video"), Some("audio"))
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
        let opus = select_tracks(&catalog, VideoChoice::Named("video"), Some("audio"))
            .unwrap()
            .unwrap();
        let pcma = select_tracks(
            &catalog,
            VideoChoice::Named("video"),
            Some("audio/profile_1"),
        )
        .unwrap()
        .unwrap();
        let mp3 = select_tracks(&catalog, VideoChoice::Named("video"), Some("audio_mp3"))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(
            opus.audio.unwrap().config,
            AudioConfig::Opus {
                sample_rate: 48_000,
                channels: 2
            }
        );
        assert_eq!(
            pcma.audio.unwrap().config,
            AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 1
            }
        );
        assert_eq!(
            mp3.audio.unwrap().config,
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
        let selection = select_tracks(&catalog, VideoChoice::Named("video"), Some("audio"))
            .unwrap()
            .unwrap();
        let error = select_tracks(&catalog, VideoChoice::Named("hevc"), Some("audio")).unwrap_err();

        // Assert
        assert_eq!(selection.video.codec, VideoCodec::Av1);
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
            select_tracks(&catalog, VideoChoice::Named("video"), Some("audio")).unwrap(),
            None
        );
    }
}

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose};
use mediapack::{AudioConfig, aac::AudioSpecificConfig};
use msf::{Catalog, KnownPackaging, Packaging, Track};

#[derive(Debug, PartialEq)]
pub(crate) struct Selection {
    pub(crate) video: String,
    pub(crate) audio: Option<AudioTrack>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct AudioTrack {
    pub(crate) name: String,
    pub(crate) config: AudioConfig,
}

/// `Ok(None)` until the catalog lists the video track, which a broadcast does
/// from its first keyframe. An audio track listed without its decoder
/// configuration is dropped with a warning: `broadcast` lists AAC audio before
/// the first audio frame delivers its configuration.
pub(crate) fn select_tracks(
    catalog: &Catalog,
    video_name: &str,
    audio_name: &str,
) -> Result<Option<Selection>> {
    let Some(video) = loc_track(catalog, video_name) else {
        return Ok(None);
    };
    let audio = match loc_track(catalog, audio_name) {
        Some(track) => match audio_config(catalog, track)? {
            Some(config) => Some(AudioTrack {
                name: track.name.clone(),
                config,
            }),
            None => {
                tracing::warn!(
                    track = audio_name,
                    codec = track.codec.as_deref().unwrap_or("-"),
                    "audio track cannot be decoded from the catalog; recording video only"
                );
                None
            }
        },
        None => {
            tracing::warn!(
                track = audio_name,
                "catalog lists no LOC audio track; recording video only"
            );
            None
        }
    };
    Ok(Some(Selection {
        video: video.name.clone(),
        audio,
    }))
}

fn loc_track<'a>(catalog: &'a Catalog, name: &str) -> Option<&'a Track> {
    catalog.tracks.as_deref()?.iter().find(|track| {
        track.name == name && track.packaging == Packaging::Known(KnownPackaging::Loc)
    })
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
        Track::new("video", KnownPackaging::Loc, true)
    }

    fn audio_track(codec: &str) -> Track {
        let mut track = Track::new("audio", KnownPackaging::Loc, true);
        track.codec = Some(codec.to_string());
        track.sample_rate = Some(48_000);
        track.channel_config = Some("stereo".to_string());
        track
    }

    fn catalog(tracks: Vec<Track>) -> Catalog {
        Catalog::new(tracks, 0)
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
        let selection = select_tracks(&catalog, "video", "audio").unwrap();

        // Assert
        assert_eq!(
            selection,
            Some(Selection {
                video: "video".to_string(),
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
        assert_eq!(select_tracks(&catalog, "video", "audio").unwrap(), None);
    }

    #[test]
    fn records_video_only_when_the_aac_track_has_no_init_data_yet() {
        // Arrange
        let catalog = catalog(vec![video_track(), audio_track("mp4a.40.2")]);

        // Act
        let selection = select_tracks(&catalog, "video", "audio").unwrap().unwrap();

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
        let opus = select_tracks(&catalog, "video", "audio").unwrap().unwrap();
        let pcma = select_tracks(&catalog, "video", "audio/profile_1")
            .unwrap()
            .unwrap();
        let mp3 = select_tracks(&catalog, "video", "audio_mp3")
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
    fn only_loc_tracks_qualify() {
        // Arrange
        let mut cmaf = video_track();
        cmaf.packaging = Packaging::Known(KnownPackaging::Cmaf);
        let catalog = catalog(vec![cmaf]);

        // Act / Assert
        assert_eq!(select_tracks(&catalog, "video", "audio").unwrap(), None);
    }
}

use msf::{Catalog, KnownPackaging, KnownTrackRole, Packaging, Track, TrackRole};

use crate::{opus_codec::TRANSLATED_BITRATE, target_language::TargetLanguage};

const SOURCE_LANGUAGE: &str = "ja";
const LANGUAGE_ALT_GROUP: u64 = 1;

pub(crate) fn source_audio_track(source: &Catalog) -> Option<&Track> {
    source
        .tracks
        .iter()
        .flatten()
        .filter(|track| is_opus_audio(track))
        .max_by_key(|track| track.bitrate.unwrap_or(0))
}

pub(crate) fn build_translated_catalog(
    source: &Catalog,
    source_namespace: &str,
    languages: &[TargetLanguage],
) -> Option<Catalog> {
    let source_audio = source_audio_track(source)?;
    let video = source
        .tracks
        .iter()
        .flatten()
        .filter(|track| has_role(track, KnownTrackRole::Video))
        .map(|track| referencing(track, source_namespace));
    let original_audio = source
        .tracks
        .iter()
        .flatten()
        .filter(|track| is_opus_audio(track))
        .map(|track| Track {
            lang: Some(SOURCE_LANGUAGE.to_string()),
            alt_group: Some(LANGUAGE_ALT_GROUP),
            ..referencing(track, source_namespace)
        });
    let translated = languages.iter().map(|language| Track {
        namespace: None,
        name: language.track_name(),
        label: Some(language.code.to_string()),
        lang: Some(language.code.to_string()),
        alt_group: Some(LANGUAGE_ALT_GROUP),
        bitrate: Some(u64::from(TRANSLATED_BITRATE)),
        init_ref: None,
        ..source_audio.clone()
    });
    Some(Catalog {
        version: source.version,
        delta_update: None,
        add_tracks: None,
        remove_tracks: None,
        clone_tracks: None,
        generated_at: None,
        is_complete: None,
        init_data_list: None,
        tracks: Some(video.chain(original_audio).chain(translated).collect()),
    })
}

fn referencing(track: &Track, source_namespace: &str) -> Track {
    Track {
        namespace: Some(
            track
                .namespace
                .clone()
                .unwrap_or_else(|| source_namespace.to_string()),
        ),
        ..track.clone()
    }
}

fn is_opus_audio(track: &Track) -> bool {
    has_role(track, KnownTrackRole::Audio)
        && track.packaging == Packaging::Known(KnownPackaging::Loc)
        && track.codec.as_deref() == Some("opus")
}

fn has_role(track: &Track, role: KnownTrackRole) -> bool {
    track.role == Some(TrackRole::Known(role))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target_language::TARGET_LANGUAGES;

    const SOURCE_NAMESPACE: &str = "anon/live-translate/brave-otter";

    fn browser_publisher_catalog() -> Catalog {
        serde_json::from_str(
            r#"{ "version": 1, "tracks": [
              { "namespace": "anon/live-translate/brave-otter", "name": "video_720p", "packaging": "loc", "role": "video", "isLive": true, "codec": "avc1.640028", "width": 1280, "height": 720 },
              { "namespace": "anon/live-translate/brave-otter", "name": "audio_64kbps", "packaging": "loc", "role": "audio", "isLive": true, "codec": "opus", "bitrate": 64000, "samplerate": 48000, "channelConfig": "mono" },
              { "namespace": "anon/live-translate/brave-otter", "name": "audio_128kbps", "packaging": "loc", "role": "audio", "isLive": true, "codec": "opus", "bitrate": 128000, "samplerate": 48000, "channelConfig": "mono" }
            ]}"#,
        )
        .expect("valid catalog")
    }

    fn track<'a>(catalog: &'a Catalog, name: &str) -> &'a Track {
        catalog
            .tracks
            .iter()
            .flatten()
            .find(|track| track.name == name)
            .expect("track exists")
    }

    #[test]
    fn highest_bitrate_opus_track_is_the_translation_source() {
        // Arrange
        let source = browser_publisher_catalog();

        // Act
        let selected = source_audio_track(&source);

        // Assert
        assert_eq!(
            selected.map(|track| track.name.as_str()),
            Some("audio_128kbps")
        );
    }

    #[test]
    fn original_tracks_keep_pointing_at_the_broadcast_namespace() {
        // Arrange
        let source = browser_publisher_catalog();

        // Act
        let catalog = build_translated_catalog(&source, SOURCE_NAMESPACE, &TARGET_LANGUAGES)
            .expect("source has audio");

        // Assert
        assert_eq!(
            track(&catalog, "video_720p").namespace.as_deref(),
            Some(SOURCE_NAMESPACE)
        );
        let original = track(&catalog, "audio_128kbps");
        assert_eq!(original.namespace.as_deref(), Some(SOURCE_NAMESPACE));
        assert_eq!(original.lang.as_deref(), Some("ja"));
        assert_eq!(original.alt_group, Some(1));
    }

    #[test]
    fn every_target_language_is_listed_in_the_translated_namespace() {
        // Arrange
        let source = browser_publisher_catalog();

        // Act
        let catalog = build_translated_catalog(&source, SOURCE_NAMESPACE, &TARGET_LANGUAGES)
            .expect("source has audio");

        // Assert
        let english = track(&catalog, "audio_en");
        assert_eq!(english.namespace, None);
        assert_eq!(english.lang.as_deref(), Some("en"));
        assert_eq!(english.alt_group, Some(1));
        assert_eq!(english.codec.as_deref(), Some("opus"));
        assert_eq!(english.bitrate, Some(32_000));
        assert_eq!(
            catalog.tracks.as_ref().map(Vec::len),
            Some(3 + TARGET_LANGUAGES.len())
        );
    }

    #[test]
    fn catalog_without_opus_audio_has_nothing_to_translate() {
        // Arrange
        let mut source = browser_publisher_catalog();
        source.tracks = source.tracks.map(|tracks| {
            tracks
                .into_iter()
                .filter(|track| track.role == Some(TrackRole::Known(KnownTrackRole::Video)))
                .collect()
        });

        // Act / Assert
        assert!(build_translated_catalog(&source, SOURCE_NAMESPACE, &TARGET_LANGUAGES).is_none());
    }
}

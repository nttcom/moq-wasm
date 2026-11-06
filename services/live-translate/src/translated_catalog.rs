use msf::{Catalog, KnownPackaging, KnownTrackRole, Packaging, Track, TrackRole};

use crate::{
    opus_codec::TRANSLATED_BITRATE,
    target_language::{SOURCE_TEXT_TRACK, TARGET_LANGUAGES},
};

const SOURCE_LANGUAGE: &str = "ja";
const LANGUAGE_ALT_GROUP: u64 = 1;
const TEXT_ALT_GROUP: u64 = 2;
/// A transcript object is JSON `{"text": "...", "final": bool}`.
const TEXT_MIME_TYPE: &str = "application/json";

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
    let translated = TARGET_LANGUAGES.iter().map(|language| Track {
        namespace: None,
        name: language.track_name(),
        label: Some(language.code.to_string()),
        lang: Some(language.code.to_string()),
        alt_group: Some(LANGUAGE_ALT_GROUP),
        bitrate: Some(u64::from(TRANSLATED_BITRATE)),
        init_ref: None,
        ..source_audio.clone()
    });
    let texts = std::iter::once((SOURCE_TEXT_TRACK.to_string(), SOURCE_LANGUAGE))
        .chain(
            TARGET_LANGUAGES
                .iter()
                .map(|language| (language.text_track_name(), language.code)),
        )
        .map(|(name, lang)| text_track(source_audio, name, lang));
    let tracks = video
        .chain(original_audio)
        .chain(translated)
        .chain(texts)
        .collect();
    Some(Catalog {
        version: source.version,
        generated_at: None,
        ..Catalog::new(tracks, 0)
    })
}

/// `lang` equals the audio track's, so a viewer pairs a transcript with its audio.
fn text_track(source_audio: &Track, name: String, lang: &str) -> Track {
    Track {
        namespace: None,
        name,
        packaging: Packaging::Known(KnownPackaging::Loc),
        role: Some(TrackRole::Known(KnownTrackRole::Subtitle)),
        label: Some(lang.to_string()),
        lang: Some(lang.to_string()),
        alt_group: Some(TEXT_ALT_GROUP),
        codec: None,
        mime_type: Some(TEXT_MIME_TYPE.to_string()),
        bitrate: None,
        sample_rate: None,
        channel_config: None,
        init_ref: None,
        ..source_audio.clone()
    }
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
        let catalog =
            build_translated_catalog(&source, SOURCE_NAMESPACE).expect("source has audio");

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
        let catalog =
            build_translated_catalog(&source, SOURCE_NAMESPACE).expect("source has audio");

        // Assert
        let english = track(&catalog, "audio_en");
        assert_eq!(english.namespace, None);
        assert_eq!(english.lang.as_deref(), Some("en"));
        assert_eq!(english.alt_group, Some(1));
        assert_eq!(english.codec.as_deref(), Some("opus"));
        assert_eq!(english.bitrate, Some(32_000));
        assert_eq!(
            catalog.tracks.as_ref().map(Vec::len),
            Some(3 + TARGET_LANGUAGES.len() + 1 + TARGET_LANGUAGES.len())
        );
    }

    #[test]
    fn each_language_and_the_source_get_a_transcript_track() {
        // Arrange
        let source = browser_publisher_catalog();

        // Act
        let catalog =
            build_translated_catalog(&source, SOURCE_NAMESPACE).expect("source has audio");

        // Assert: text tracks are subtitles paired with the audio by lang, without audio fields
        let english = track(&catalog, "text_en");
        assert_eq!(
            english.role,
            Some(TrackRole::Known(KnownTrackRole::Subtitle))
        );
        assert_eq!(english.lang.as_deref(), Some("en"));
        assert_eq!(english.alt_group, Some(2));
        assert_eq!(english.mime_type.as_deref(), Some("application/json"));
        assert_eq!(english.codec, None);
        assert_eq!(english.bitrate, None);
        let japanese = track(&catalog, "text_ja");
        assert_eq!(japanese.lang.as_deref(), Some("ja"));
        assert_eq!(japanese.namespace, None);
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
        assert!(build_translated_catalog(&source, SOURCE_NAMESPACE).is_none());
    }
}

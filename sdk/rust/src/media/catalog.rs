use bytes::Bytes;
use msf::Track;

/// An independent draft-ietf-moq-msf-02 catalog (§5).
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    catalog: msf::Catalog,
}

/// Which tracks `Catalog::select` takes: every field given must match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackQuery<'a> {
    /// The `role`, such as `video`, `audio` or `eventtimeline`.
    pub role: Option<&'a str>,
    /// The `packaging`, such as `loc` or `eventtimeline`.
    pub packaging: Option<&'a str>,
    /// The codec string or its first element: `avc1` matches `avc1.42e01f`.
    pub codec: Option<&'a str>,
    /// Which matching track to take; `None` takes the first in the catalog.
    pub prefer: Option<BitratePreference>,
}

/// Which bitrate `TrackQuery::prefer` picks. A track without a bitrate,
/// which draft-ietf-moq-msf-02 requires only of audio and video, counts as
/// the highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitratePreference {
    Lowest,
    Highest,
}

impl Catalog {
    /// A catalog of `tracks` with `version` `draft-02` (draft-ietf-moq-msf-02
    /// §5.1.1). `generatedAt` (§5.1.2) and `isComplete` (§5.1.3) are left
    /// absent: both are optional, and `isComplete` commits the broadcast to
    /// publish nothing more. A track with a `namespace` (§5.2.2) is read from
    /// that namespace instead of the catalog's. `tracks` are not validated;
    /// `parse` refuses a catalog the draft calls invalid.
    pub fn new(tracks: Vec<Track>) -> Self {
        Self {
            catalog: msf::Catalog {
                generated_at: None,
                ..msf::Catalog::new(tracks, 0)
            },
        }
    }

    pub fn parse(payload: &[u8]) -> Result<Self, msf::CatalogParseError> {
        Ok(Self {
            catalog: msf::Catalog::parse(payload)?,
        })
    }

    pub fn tracks(&self) -> &[Track] {
        &self.catalog.tracks
    }

    /// The catalog's JSON (draft-ietf-moq-msf-02 §5), to write as one group
    /// of the `catalog` track.
    pub fn to_bytes(&self) -> Bytes {
        // Every field serializes as a JSON string, number, bool, array or
        // string-keyed object, the only shapes serde_json writes without error.
        let json = serde_json::to_vec(&self.catalog).expect("an MSF catalog serializes to JSON");
        Bytes::from(json)
    }

    pub fn select(&self, query: &TrackQuery) -> Option<&Track> {
        let mut matching = self.tracks().iter().filter(|track| {
            // A role or packaging is compared as it is written; an absent role is
            // `null`, so it matches none.
            query.role.is_none_or(|role| {
                serde_json::to_value(&track.role).is_ok_and(|value| value == role)
            }) && query.packaging.is_none_or(|packaging| {
                serde_json::to_value(&track.packaging).is_ok_and(|value| value == packaging)
            }) && query.codec.is_none_or(|codec| {
                track
                    .codec
                    .as_deref()
                    .is_some_and(|own| own == codec || own.split('.').next() == Some(codec))
            })
        });
        let bitrate = |track: &&Track| track.bitrate.unwrap_or(u64::MAX);
        match query.prefer {
            None => matching.next(),
            Some(BitratePreference::Lowest) => matching.min_by_key(bitrate),
            Some(BitratePreference::Highest) => matching.max_by_key(bitrate),
        }
    }
}

#[cfg(test)]
mod tests {
    use msf::{KnownPackaging, KnownTrackRole, TrackRole};

    use super::*;

    fn catalog() -> Catalog {
        let payload = serde_json::json!({
            "version": "draft-02",
            "tracks": [
                {"name": "video", "packaging": "loc", "role": "video", "isLive": true,
                 "codec": "avc1.42e01f", "bitrate": 2_000_000, "width": 1280, "height": 720,
                 "framerate": 30},
                {"name": "video_low", "packaging": "loc", "role": "video", "isLive": true,
                 "codec": "avc1.42e01e", "bitrate": 300_000, "width": 320, "height": 180,
                 "framerate": 30},
                {"name": "audio", "packaging": "loc", "role": "audio", "isLive": true,
                 "codec": "opus", "bitrate": 64_000, "samplerate": 48_000,
                 "channelConfig": "1"},
            ],
        });
        Catalog::parse(payload.to_string().as_bytes()).unwrap()
    }

    #[test]
    fn a_built_catalog_parses_back_with_its_tracks() {
        // Arrange
        let mut source_video = Track::new("video", KnownPackaging::Loc, true);
        source_video.namespace = Some("live/source".to_string());
        source_video.role = Some(TrackRole::Known(KnownTrackRole::Video));
        source_video.codec = Some("avc1.42e01f".to_string());
        source_video.bitrate = Some(2_000_000);
        source_video.width = Some(1280);
        source_video.height = Some(720);
        source_video.framerate = Some(30.0);
        let mut text = Track::new("text", KnownPackaging::Loc, true);
        text.role = Some(TrackRole::Known(KnownTrackRole::Subtitle));
        text.mime_type = Some("text/plain".to_string());
        let catalog = Catalog::new(vec![source_video, text]);

        // Act
        let parsed = Catalog::parse(&catalog.to_bytes()).unwrap();

        // Assert
        assert_eq!(parsed, catalog);
    }

    #[test]
    fn select_takes_the_first_match_or_the_preferred_bitrate() {
        // Arrange
        let catalog = catalog();
        let video = |prefer| TrackQuery {
            role: Some("video"),
            prefer,
            ..TrackQuery::default()
        };

        // Act
        let first = catalog.select(&video(None));
        let lowest = catalog.select(&video(Some(BitratePreference::Lowest)));
        let highest = catalog.select(&video(Some(BitratePreference::Highest)));

        // Assert
        assert_eq!(first.map(|track| track.name.as_str()), Some("video"));
        assert_eq!(lowest.map(|track| track.name.as_str()), Some("video_low"));
        assert_eq!(highest.map(|track| track.name.as_str()), Some("video"));
    }

    #[test]
    fn a_codec_matches_its_first_element_and_every_field_must_match() {
        // Arrange
        let catalog = catalog();

        // Act
        let by_codec = catalog.select(&TrackQuery {
            codec: Some("opus"),
            packaging: Some("loc"),
            ..TrackQuery::default()
        });
        let avc = catalog.select(&TrackQuery {
            codec: Some("avc1"),
            role: Some("audio"),
            ..TrackQuery::default()
        });

        // Assert
        assert_eq!(by_codec.map(|track| track.name.as_str()), Some("audio"));
        assert!(avc.is_none());
    }
}

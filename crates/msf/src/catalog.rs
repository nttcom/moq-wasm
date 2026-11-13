use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::track::Track;
use crate::types::{KnownPackaging, Packaging};

/// draft-ietf-moq-msf-01 §5: the catalog track MUST have this name.
pub const CATALOG_TRACK_NAME: &str = "catalog";

/// The catalog `version` (draft-ietf-moq-msf-01 §5.1.1).
const MSF_VERSION: &str = "draft-01";

/// An independent catalog (draft-ietf-moq-msf-01 §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<Track>>,
    /// Declared after `tracks`: serde writes the fields in declaration order,
    /// and draft-ietf-moq-msf-01 §5.1.7 requires the list to follow the
    /// tracks array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_data_list: Option<Vec<InitData>>,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum CatalogParseError {
    /// Not JSON, or not an MSF catalog.
    Malformed(serde_json::Error),
    /// A `version` this crate does not understand, or none; draft-ietf-moq-msf-01
    /// §5.1.1 forbids parsing it.
    UnsupportedVersion(Option<String>),
    /// A delta update (§5.1.6).
    DeltaUpdate,
}

impl fmt::Display for CatalogParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "not an MSF catalog: {error}"),
            Self::UnsupportedVersion(Some(version)) => {
                write!(f, "MSF catalog version {version} is not {MSF_VERSION}")
            }
            Self::UnsupportedVersion(None) => write!(f, "MSF catalog has no version"),
            Self::DeltaUpdate => write!(f, "MSF catalog delta updates are not supported"),
        }
    }
}

impl std::error::Error for CatalogParseError {}

/// An entry of `initDataList` (draft-ietf-moq-msf-01 §5.1.7), named by the
/// `initRef` of the tracks it initializes (§5.2.13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitData {
    pub id: String,
    /// `inline`, the only type the draft defines, carries Base64 `data`.
    pub r#type: String,
    pub data: String,
}

impl Catalog {
    /// `isComplete` is left absent: draft-ietf-moq-msf-01 §5.1.3 reserves it
    /// for a broadcast that will publish nothing more.
    pub fn new(tracks: Vec<Track>, generated_at_ms: u64) -> Self {
        Self {
            version: Some(MSF_VERSION.to_string()),
            generated_at: Some(generated_at_ms),
            is_complete: None,
            tracks: Some(tracks),
            init_data_list: None,
        }
    }

    /// Reads a catalog object, refusing one this crate cannot read as
    /// draft-ietf-moq-msf-01 requires (§5.1.1, §5.1.6).
    pub fn parse(payload: &[u8]) -> Result<Self, CatalogParseError> {
        let document: Value =
            serde_json::from_slice(payload).map_err(CatalogParseError::Malformed)?;
        if document.get("deltaUpdate").is_some() {
            return Err(CatalogParseError::DeltaUpdate);
        }
        match document.get("version").and_then(Value::as_str) {
            Some(MSF_VERSION) => {}
            version => {
                return Err(CatalogParseError::UnsupportedVersion(
                    version.map(str::to_string),
                ));
            }
        }
        serde_json::from_value(document).map_err(CatalogParseError::Malformed)
    }

    pub fn loc_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .flatten()
            .filter(|track| track.packaging == Packaging::Known(KnownPackaging::Loc))
    }

    /// The Base64 `data` of the `initDataList` entry `track`'s `initRef`
    /// names (draft-ietf-moq-msf-01 §5.1.7, §5.2.13).
    pub fn init_data(&self, track: &Track) -> Option<&str> {
        let init_ref = track.init_ref.as_deref()?;
        self.init_data_list
            .iter()
            .flatten()
            .find(|entry| entry.id == init_ref)
            .map(|entry| entry.data.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KnownPackaging;

    #[test]
    fn parse_reads_a_draft_01_catalog() {
        // Arrange
        let catalog = Catalog::new(
            vec![Track::new("video", KnownPackaging::Loc, true)],
            1_759_924_160_383,
        );
        let payload = serde_json::to_vec(&catalog).unwrap();

        // Act / Assert
        assert_eq!(Catalog::parse(&payload).unwrap(), catalog);
    }

    #[test]
    fn parse_refuses_a_version_it_does_not_understand_or_none() {
        // Arrange
        let draft_03 = br#"{"version":"draft-03","tracks":[]}"#;
        let unversioned = br#"{"tracks":[]}"#;

        // Act
        let draft_03 = Catalog::parse(draft_03);
        let unversioned = Catalog::parse(unversioned);

        // Assert
        assert!(matches!(
            draft_03,
            Err(CatalogParseError::UnsupportedVersion(Some(version))) if version == "draft-03"
        ));
        assert!(matches!(
            unversioned,
            Err(CatalogParseError::UnsupportedVersion(None))
        ));
    }

    #[test]
    fn parse_refuses_a_delta_update() {
        // Arrange: draft-ietf-moq-msf-01 §5.1.6, with no version as §5.3 requires
        let delta = br#"{"deltaUpdate":[{"op":"remove","tracks":[{"name":"video"}]}]}"#;

        // Act / Assert
        assert!(matches!(
            Catalog::parse(delta),
            Err(CatalogParseError::DeltaUpdate)
        ));
    }

    #[test]
    fn parse_refuses_what_is_not_a_catalog() {
        // Act / Assert
        assert!(matches!(
            Catalog::parse(b"not json"),
            Err(CatalogParseError::Malformed(_))
        ));
        assert!(matches!(
            Catalog::parse(br#"{"version":"draft-01","tracks":[{"name":"video"}]}"#),
            Err(CatalogParseError::Malformed(_))
        ));
    }

    #[test]
    fn new_is_an_independent_draft_01_catalog_without_is_complete() {
        // Act
        let catalog = Catalog::new(
            vec![Track::new("video", KnownPackaging::Loc, true)],
            1_759_924_160_383,
        );

        // Assert
        assert_eq!(
            serde_json::to_string(&catalog).unwrap(),
            r#"{"version":"draft-01","generatedAt":1759924160383,"tracks":[{"name":"video","packaging":"loc","isLive":true}]}"#
        );
    }

    #[test]
    fn init_data_list_follows_the_tracks_array() {
        // Arrange
        let mut track = Track::new("audio", KnownPackaging::Loc, true);
        let init_data = track.set_inline_init_data("ERA=".to_string());
        let mut catalog = Catalog::new(vec![track], 1_759_924_160_383);
        catalog.init_data_list = Some(vec![init_data]);

        // Act
        let json = serde_json::to_string(&catalog).unwrap();

        // Assert
        assert_eq!(
            json,
            r#"{"version":"draft-01","generatedAt":1759924160383,"tracks":[{"name":"audio","packaging":"loc","isLive":true,"initRef":"audio"}],"initDataList":[{"id":"audio","type":"inline","data":"ERA="}]}"#
        );
    }

    #[test]
    fn a_catalog_with_unresolvable_init_data_still_parses() {
        // Arrange: the list precedes the tracks, its entry has a type the draft does not define, and the initRef names no entry
        let json = r#"{"version":"draft-01","initDataList":[{"id":"init-url","type":"url","data":"https://example.com/init.mp4"}],"tracks":[{"name":"video","packaging":"cmaf","isLive":true,"initRef":"init-hd"}]}"#;

        // Act
        let catalog: Catalog = serde_json::from_str(json).unwrap();

        // Assert
        assert_eq!(catalog.init_data_list.unwrap()[0].r#type, "url");
        assert_eq!(
            catalog.tracks.unwrap()[0].init_ref.as_deref(),
            Some("init-hd")
        );
    }
}

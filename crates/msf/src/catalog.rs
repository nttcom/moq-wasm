use serde::{Deserialize, Serialize};

use crate::track::{Track, TrackRef};

/// draft-ietf-moq-msf-01 §5: the catalog track MUST have this name.
pub const CATALOG_TRACK_NAME: &str = "catalog";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_update: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_tracks: Option<Vec<Track>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_tracks: Option<Vec<TrackRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clone_tracks: Option<Vec<Track>>,
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
            version: Some(1),
            delta_update: None,
            add_tracks: None,
            remove_tracks: None,
            clone_tracks: None,
            generated_at: Some(generated_at_ms),
            is_complete: None,
            tracks: Some(tracks),
            init_data_list: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KnownPackaging;

    #[test]
    fn new_is_an_independent_version_1_catalog_without_is_complete() {
        // Act
        let catalog = Catalog::new(
            vec![Track::new("video", KnownPackaging::Loc, true)],
            1_759_924_160_383,
        );

        // Assert
        assert_eq!(
            serde_json::to_string(&catalog).unwrap(),
            r#"{"version":1,"generatedAt":1759924160383,"tracks":[{"name":"video","packaging":"loc","isLive":true}]}"#
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
            r#"{"version":1,"generatedAt":1759924160383,"tracks":[{"name":"audio","packaging":"loc","isLive":true,"initRef":"audio"}],"initDataList":[{"id":"audio","type":"inline","data":"ERA="}]}"#
        );
    }

    #[test]
    fn a_catalog_with_unresolvable_init_data_still_parses() {
        // Arrange: the list precedes the tracks, its entry has a type the draft does not define, and the initRef names no entry
        let json = r#"{"version":1,"initDataList":[{"id":"init-url","type":"url","data":"https://example.com/init.mp4"}],"tracks":[{"name":"video","packaging":"cmaf","isLive":true,"initRef":"init-hd"}]}"#;

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

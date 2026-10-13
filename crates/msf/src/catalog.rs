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
}

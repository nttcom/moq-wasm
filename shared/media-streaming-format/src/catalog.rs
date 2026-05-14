use serde::{Deserialize, Serialize};

use crate::track::{Track, TrackRef};

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

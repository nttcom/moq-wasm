use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownPackaging {
    Loc,
    MediaTimeline,
    EventTimeline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Packaging {
    Known(KnownPackaging),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownTrackRole {
    AudioDescription,
    Video,
    Audio,
    MediaTimeline,
    EventTimeline,
    Caption,
    Subtitle,
    SignLanguage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TrackRole {
    Known(KnownTrackRole),
    Other(String),
}

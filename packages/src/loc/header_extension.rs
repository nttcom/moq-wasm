use serde::{Deserialize, Serialize};

pub const LOC_CAPTURE_TIMESTAMP_ID: u64 = 2;
pub const LOC_VIDEO_FRAME_MARKING_ID: u64 = 4;
pub const LOC_AUDIO_LEVEL_ID: u64 = 6;
pub const LOC_VIDEO_CONFIG_ID: u64 = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum LocHeaderExtension {
    CaptureTimestamp(CaptureTimestamp),
    VideoConfig(VideoConfig),
    VideoFrameMarking(VideoFrameMarking),
    AudioLevel(AudioLevel),
    Unknown(UnknownHeaderExtension),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocHeaderValue {
    EvenBytes(Vec<u8>),
    OddVarint(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTimestamp {
    pub micros_since_unix_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConfig {
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFrameMarking {
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioLevel {
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownHeaderExtension {
    pub id: u64,
    pub value: LocHeaderValue,
}

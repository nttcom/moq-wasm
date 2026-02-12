use serde::{Deserialize, Serialize};

pub mod header_extension;
pub use header_extension::{
    AudioLevel, CaptureTimestamp, LocHeaderExtension, LocHeaderValue,
    UnknownHeaderExtension, VideoConfig, VideoFrameMarking, LOC_AUDIO_LEVEL_ID,
    LOC_CAPTURE_TIMESTAMP_ID, LOC_VIDEO_CONFIG_ID, LOC_VIDEO_FRAME_MARKING_ID,
};
#[cfg(feature = "wasm")]
pub mod wasm;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocHeader {
    pub extensions: Vec<LocHeaderExtension>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocObject {
    pub header: LocHeader,

    pub payload: Vec<u8>,
}

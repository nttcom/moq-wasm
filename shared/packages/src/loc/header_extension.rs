use bytes::Bytes;
use moqt::{KeyValuePair, VariantType};
use serde::{Deserialize, Serialize};

pub const LOC_CAPTURE_TIMESTAMP_ID: u64 = 2;
pub const LOC_VIDEO_FRAME_MARKING_ID: u64 = 4;
pub const LOC_AUDIO_LEVEL_ID: u64 = 6;
pub const LOC_VIDEO_CONFIG_ID: u64 = 13;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum LocHeaderExtension {
    CaptureTimestamp(CaptureTimestamp),
    VideoConfig(VideoConfig),
    VideoFrameMarking(VideoFrameMarking),
    AudioLevel(AudioLevel),
    Unknown(UnknownHeaderExtension),
}

// Value carried by an extension header whose id the LOC layer does not model.
// Per draft-ietf-moq-loc-01 §2.3, even ids carry a varint value and odd ids
// carry a length-prefixed byte string, mirroring `moqt::VariantType`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocHeaderValue {
    Even(u64),
    Odd(Vec<u8>),
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
    pub flags: u64,
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

impl LocHeaderExtension {
    pub fn to_key_value_pair(&self) -> KeyValuePair {
        match self {
            LocHeaderExtension::CaptureTimestamp(ext) => KeyValuePair {
                key: LOC_CAPTURE_TIMESTAMP_ID,
                value: VariantType::Even(ext.micros_since_unix_epoch),
            },
            LocHeaderExtension::VideoFrameMarking(ext) => KeyValuePair {
                key: LOC_VIDEO_FRAME_MARKING_ID,
                value: VariantType::Even(ext.flags),
            },
            LocHeaderExtension::AudioLevel(ext) => KeyValuePair {
                key: LOC_AUDIO_LEVEL_ID,
                value: VariantType::Even(ext.level as u64),
            },
            LocHeaderExtension::VideoConfig(ext) => KeyValuePair {
                key: LOC_VIDEO_CONFIG_ID,
                value: VariantType::Odd(Bytes::from(ext.data.clone())),
            },
            LocHeaderExtension::Unknown(ext) => KeyValuePair {
                key: ext.id,
                value: match &ext.value {
                    LocHeaderValue::Even(value) => VariantType::Even(*value),
                    LocHeaderValue::Odd(value) => VariantType::Odd(Bytes::from(value.clone())),
                },
            },
        }
    }

    pub fn from_key_value_pair(kv_pair: &KeyValuePair) -> Self {
        match (kv_pair.key, &kv_pair.value) {
            (LOC_CAPTURE_TIMESTAMP_ID, VariantType::Even(value)) => {
                LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
                    micros_since_unix_epoch: *value,
                })
            }
            (LOC_VIDEO_FRAME_MARKING_ID, VariantType::Even(value)) => {
                LocHeaderExtension::VideoFrameMarking(VideoFrameMarking { flags: *value })
            }
            (LOC_AUDIO_LEVEL_ID, VariantType::Even(value)) => {
                LocHeaderExtension::AudioLevel(AudioLevel {
                    level: *value as u8,
                })
            }
            (LOC_VIDEO_CONFIG_ID, VariantType::Odd(value)) => {
                LocHeaderExtension::VideoConfig(VideoConfig {
                    data: value.to_vec(),
                })
            }
            (id, value) => LocHeaderExtension::Unknown(UnknownHeaderExtension {
                id,
                value: match value {
                    VariantType::Even(value) => LocHeaderValue::Even(*value),
                    VariantType::Odd(value) => LocHeaderValue::Odd(value.to_vec()),
                },
            }),
        }
    }
}

use bytes::Bytes;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::sample::{AudioLevel, Timestamp};

// draft-ietf-moq-loc-04 §6.1 property ids.
pub const TIMESCALE_ID: u64 = 0x08;
pub const AUDIO_LEVEL_ID: u64 = 0x0C;
pub const VIDEO_CONFIG_ID: u64 = 0x0D;
pub const TIMESTAMP_ID: u64 = 0x10;

/// draft-ietf-moq-loc-04 §2.3: an even id carries a varint value and an odd id
/// carries a length-prefixed byte string.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum LocValue {
    Varint(u64),
    Bytes(Bytes),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LocExtension {
    pub id: u64,
    pub value: LocValue,
}

impl LocExtension {
    pub fn varint(id: u64, value: u64) -> Self {
        Self {
            id,
            value: LocValue::Varint(value),
        }
    }

    pub fn bytes(id: u64, value: Bytes) -> Self {
        Self {
            id,
            value: LocValue::Bytes(value),
        }
    }
}

/// A MoQT object as described by draft-ietf-moq-loc-04 §2.2: the payload is the
/// codec's elementary bitstream and the extensions are the LOC public
/// properties carried by the MoQT object header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocObject {
    pub extensions: Vec<LocExtension>,
    pub payload: Bytes,
}

impl LocObject {
    /// The Timestamp property read as wall-clock µs since the Unix epoch
    /// (draft-ietf-moq-loc-04 §2.3.1.1). `None` when a Timescale property makes
    /// it media time with an application-defined epoch (§2.3.1.2).
    pub fn capture_timestamp(&self) -> Option<Timestamp> {
        if self.varint(TIMESCALE_ID).is_some() {
            return None;
        }
        self.varint(TIMESTAMP_ID).map(Timestamp::from_micros)
    }

    /// The codec extradata; for H.264 this is an AVC decoder configuration record.
    pub fn video_config(&self) -> Option<Bytes> {
        self.bytes(VIDEO_CONFIG_ID)
    }

    /// Decodes the property's least significant 8 bits (draft-ietf-moq-loc-04
    /// §2.3.3.2).
    pub fn audio_level(&self) -> Option<AudioLevel> {
        self.varint(AUDIO_LEVEL_ID)
            .map(|value| AudioLevel::from_byte(value as u8))
    }

    fn varint(&self, id: u64) -> Option<u64> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                LocExtension {
                    id: found,
                    value: LocValue::Varint(value),
                } if *found == id => Some(*value),
                _ => None,
            })
    }

    fn bytes(&self, id: u64) -> Option<Bytes> {
        self.extensions
            .iter()
            .find_map(|extension| match extension {
                LocExtension {
                    id: found,
                    value: LocValue::Bytes(value),
                } if *found == id => Some(value.clone()),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_known_extensions() {
        // Arrange
        let object = LocObject {
            extensions: vec![
                LocExtension::varint(TIMESTAMP_ID, 1_700_000_000_000_000),
                LocExtension::varint(AUDIO_LEVEL_ID, 42),
                LocExtension::bytes(VIDEO_CONFIG_ID, Bytes::from_static(&[1, 0x42])),
            ],
            payload: Bytes::from_static(&[9]),
        };

        // Act / Assert
        assert_eq!(
            object.capture_timestamp(),
            Some(Timestamp::from_micros(1_700_000_000_000_000))
        );
        assert_eq!(
            object.audio_level(),
            Some(AudioLevel {
                level: 42,
                voice_activity: false
            })
        );
        assert_eq!(object.video_config(), Some(Bytes::from_static(&[1, 0x42])));
    }

    #[test]
    fn reads_no_capture_timestamp_from_a_timestamp_in_media_time() {
        // Arrange
        let object = LocObject {
            extensions: vec![
                LocExtension::varint(TIMESCALE_ID, 48_000),
                LocExtension::varint(TIMESTAMP_ID, 96_000),
            ],
            payload: Bytes::new(),
        };

        // Act / Assert
        assert_eq!(object.capture_timestamp(), None);
    }

    #[test]
    fn ignores_an_extension_whose_value_type_does_not_match_its_id() {
        // Arrange
        let object = LocObject {
            extensions: vec![LocExtension::bytes(TIMESTAMP_ID, Bytes::from_static(&[0]))],
            payload: Bytes::new(),
        };

        // Act / Assert
        assert_eq!(object.capture_timestamp(), None);
    }

    #[test]
    fn reports_missing_extensions_as_absent() {
        // Arrange
        let object = LocObject::default();

        // Act / Assert
        assert_eq!(object.capture_timestamp(), None);
        assert_eq!(object.video_config(), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serializes_extensions_as_id_and_tagged_value() {
        // Arrange
        let extensions = vec![
            LocExtension::varint(TIMESTAMP_ID, 7),
            LocExtension::bytes(VIDEO_CONFIG_ID, Bytes::from_static(&[1, 2])),
        ];

        // Act
        let json = serde_json::to_string(&extensions).unwrap();

        // Assert
        assert_eq!(
            json,
            r#"[{"id":16,"value":{"varint":7}},{"id":13,"value":{"bytes":[1,2]}}]"#
        );
        assert_eq!(
            serde_json::from_str::<Vec<LocExtension>>(&json).unwrap(),
            extensions
        );
    }
}

use bytes::Bytes;

use crate::sample::Timestamp;

pub const CAPTURE_TIMESTAMP_ID: u64 = 2;
pub const VIDEO_FRAME_MARKING_ID: u64 = 4;
pub const AUDIO_LEVEL_ID: u64 = 6;
pub const VIDEO_CONFIG_ID: u64 = 13;

/// draft-ietf-moq-loc-01 §2.3: an even id carries a varint value and an odd id
/// carries a length-prefixed byte string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocValue {
    Varint(u64),
    Bytes(Bytes),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A MoQT object as described by draft-ietf-moq-loc-01 §2.2: the payload is the
/// codec's elementary bitstream and the extensions are the LOC header
/// extensions carried by the MoQT object header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocObject {
    pub extensions: Vec<LocExtension>,
    pub payload: Bytes,
}

impl LocObject {
    pub fn capture_timestamp(&self) -> Option<Timestamp> {
        self.varint(CAPTURE_TIMESTAMP_ID)
            .map(Timestamp::from_micros)
    }

    /// The codec extradata; for H.264 this is an AVC decoder configuration record.
    pub fn video_config(&self) -> Option<Bytes> {
        self.bytes(VIDEO_CONFIG_ID)
    }

    /// RFC 9626 frame marking flags, left uninterpreted.
    pub fn video_frame_marking(&self) -> Option<u64> {
        self.varint(VIDEO_FRAME_MARKING_ID)
    }

    pub fn audio_level(&self) -> Option<u64> {
        self.varint(AUDIO_LEVEL_ID)
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
                LocExtension::varint(CAPTURE_TIMESTAMP_ID, 1_700_000_000_000_000),
                LocExtension::varint(VIDEO_FRAME_MARKING_ID, 0xE0),
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
        assert_eq!(object.video_frame_marking(), Some(0xE0));
        assert_eq!(object.audio_level(), Some(42));
        assert_eq!(object.video_config(), Some(Bytes::from_static(&[1, 0x42])));
    }

    #[test]
    fn ignores_an_extension_whose_value_type_does_not_match_its_id() {
        // Arrange
        let object = LocObject {
            extensions: vec![LocExtension::bytes(
                CAPTURE_TIMESTAMP_ID,
                Bytes::from_static(&[0]),
            )],
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
}

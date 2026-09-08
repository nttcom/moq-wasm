use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::sample::{StreamSet, Timestamp};

const FILE_HEADER_LENGTH: usize = 9;
const TAG_HEADER_LENGTH: usize = 11;
const PREVIOUS_TAG_SIZE_LENGTH: usize = 4;
const FLAG_VIDEO: u8 = 0x01;
const FLAG_AUDIO: u8 = 0x04;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    Audio,
    Video,
    Other(u8),
}

impl TagType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            8 => Self::Audio,
            9 => Self::Video,
            other => Self::Other(other),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Audio => 8,
            Self::Video => 9,
            Self::Other(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub tag_type: TagType,
    pub timestamp: Timestamp,
    pub data: Bytes,
}

pub fn file_header(streams: StreamSet) -> [u8; FILE_HEADER_LENGTH + PREVIOUS_TAG_SIZE_LENGTH] {
    let mut flags = 0;
    if streams.has_video {
        flags |= FLAG_VIDEO;
    }
    if streams.has_audio {
        flags |= FLAG_AUDIO;
    }
    [b'F', b'L', b'V', 1, flags, 0, 0, 0, 9, 0, 0, 0, 0]
}

pub fn parse_file_header(data: &[u8]) -> Result<Option<(StreamSet, usize)>> {
    if data.len() < FILE_HEADER_LENGTH {
        return Ok(None);
    }
    ensure!(&data[..3] == b"FLV", "missing FLV signature");
    ensure!(data[3] == 1, "unsupported FLV version {}", data[3]);
    let data_offset = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) as usize;
    ensure!(
        data_offset >= FILE_HEADER_LENGTH,
        "FLV data offset {data_offset} shorter than header"
    );
    let body_start = data_offset + PREVIOUS_TAG_SIZE_LENGTH;
    if data.len() < body_start {
        return Ok(None);
    }
    Ok(Some((
        StreamSet {
            has_video: data[4] & FLAG_VIDEO != 0,
            has_audio: data[4] & FLAG_AUDIO != 0,
        },
        body_start,
    )))
}

pub fn parse_tag(data: &[u8]) -> Result<Option<(Tag, usize)>> {
    if data.len() < TAG_HEADER_LENGTH {
        return Ok(None);
    }
    let data_size = u32::from_be_bytes([0, data[1], data[2], data[3]]) as usize;
    let consumed = TAG_HEADER_LENGTH + data_size + PREVIOUS_TAG_SIZE_LENGTH;
    if data.len() < consumed {
        return Ok(None);
    }
    let timestamp_ms = u32::from_be_bytes([data[7], data[4], data[5], data[6]]);
    Ok(Some((
        Tag {
            tag_type: TagType::from_u8(data[0]),
            timestamp: Timestamp::from_millis(timestamp_ms as u64),
            data: Bytes::copy_from_slice(&data[TAG_HEADER_LENGTH..TAG_HEADER_LENGTH + data_size]),
        },
        consumed,
    )))
}

pub fn encode_tag(tag: &Tag) -> Bytes {
    let data_size = tag.data.len() as u32;
    let timestamp_ms = tag.timestamp.millis() as u32;
    let mut out =
        BytesMut::with_capacity(TAG_HEADER_LENGTH + tag.data.len() + PREVIOUS_TAG_SIZE_LENGTH);
    out.put_u8(tag.tag_type.to_u8());
    out.put_slice(&data_size.to_be_bytes()[1..]);
    out.put_slice(&timestamp_ms.to_be_bytes()[1..]);
    out.put_u8((timestamp_ms >> 24) as u8);
    out.put_slice(&[0, 0, 0]);
    out.put_slice(&tag.data);
    out.put_u32(TAG_HEADER_LENGTH as u32 + data_size);
    out.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_round_trips_with_extended_timestamp() {
        // Arrange
        let tag = Tag {
            tag_type: TagType::Video,
            timestamp: Timestamp::from_millis(0x0123_4567),
            data: Bytes::from_static(&[1, 2, 3]),
        };

        // Act
        let encoded = encode_tag(&tag);
        let (parsed, consumed) = parse_tag(&encoded).unwrap().unwrap();

        // Assert
        assert_eq!(parsed, tag);
        assert_eq!(consumed, encoded.len());
        assert_eq!(&encoded[encoded.len() - 4..], 14_u32.to_be_bytes());
    }

    #[test]
    fn file_header_round_trips_stream_flags() {
        // Arrange
        let streams = StreamSet {
            has_video: true,
            has_audio: false,
        };

        // Act
        let header = file_header(streams);
        let (parsed, body_start) = parse_file_header(&header).unwrap().unwrap();

        // Assert
        assert_eq!(parsed, streams);
        assert_eq!(body_start, header.len());
    }

    #[test]
    fn rejects_foreign_signature() {
        // Arrange
        let header = [b'M', b'P', b'4', 1, 0, 0, 0, 0, 9, 0, 0, 0, 0];

        // Act / Assert
        assert!(parse_file_header(&header).is_err());
    }
}

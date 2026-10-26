use std::time::Duration;

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::{
    modules::{executor, moqt::data_plane::object::decode_error::DecodeError},
    wire::{SubgroupHeader, SubgroupHeaderType, SubgroupObjectField},
};

/// One frame of a subgroup stream as draft-14 encodes it.
#[derive(Debug)]
pub(crate) enum SubgroupFrame {
    Header(SubgroupHeader),
    Object {
        field: SubgroupObjectField,
        /// From the first byte of the object being buffered to its last.
        receive_time: Duration,
    },
}

#[derive(Debug)]
pub(crate) struct SubgroupDecoder {
    subgroup_header_type: Option<SubgroupHeaderType>,
    /// When the first byte of the next object was buffered, on the
    /// `executor::now` clock.
    object_buffered_at: Option<Duration>,
}

impl Decoder for SubgroupDecoder {
    type Item = SubgroupFrame;
    // TODO: define a proper error type.
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.subgroup_header_type.is_none() {
            let mut cursor_buf = std::io::Cursor::new(&src[..]);
            let subgroup = self.decode_header(&mut cursor_buf)?;
            let pos = cursor_buf.position() as usize;
            let _ = src.split_to(pos);
            self.object_buffered_at = (!src.is_empty()).then(executor::now);
            Ok(subgroup)
        } else {
            self.decode_object_field(src)
        }
    }
}

impl SubgroupDecoder {
    pub(crate) fn new() -> Self {
        Self {
            subgroup_header_type: None,
            object_buffered_at: None,
        }
    }

    fn decode_header(
        &mut self,
        cursor: &mut std::io::Cursor<&[u8]>,
    ) -> Result<Option<SubgroupFrame>, std::io::Error> {
        // The first frame should always be a subgroup header.
        let subgroup_header = match SubgroupHeader::decode(cursor) {
            Ok(header) => header,
            Err(e) => {
                match e {
                    DecodeError::NeedMoreData => {
                        // Not enough data to decode the header, wait for more data.
                        return Ok(None);
                    }
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Failed to decode subgroup header: {:?}", e),
                        ));
                    }
                }
            }
        };
        self.subgroup_header_type = Some(subgroup_header.message_type);
        Ok(Some(SubgroupFrame::Header(subgroup_header)))
    }

    fn decode_object_field(
        &mut self,
        src: &mut BytesMut,
    ) -> Result<Option<SubgroupFrame>, std::io::Error> {
        if src.is_empty() {
            return Ok(None);
        }
        let buffered_at = *self.object_buffered_at.get_or_insert_with(executor::now);
        let subgroup_object_field =
            match SubgroupObjectField::decode(self.subgroup_header_type.unwrap(), src) {
                Ok(field) => field,
                Err(e) => {
                    match e {
                        DecodeError::NeedMoreData => {
                            // Not enough data to decode the object field, wait for more data.
                            return Ok(None);
                        }
                        _ => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("Failed to decode subgroup object field: {:?}", e),
                            ));
                        }
                    }
                }
            };
        let decoded_at = executor::now();
        // Bytes left over arrived with this object's last bytes.
        self.object_buffered_at = (!src.is_empty()).then_some(decoded_at);
        Ok(Some(SubgroupFrame::Object {
            field: subgroup_object_field,
            receive_time: decoded_at.saturating_sub(buffered_at),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::Decoder;

    use super::{SubgroupDecoder, SubgroupFrame};
    use crate::{
        modules::moqt::data_plane::object::extension_headers::ExtensionHeaders,
        wire::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
    };

    const GAP: Duration = Duration::from_millis(20);

    fn header() -> SubgroupHeader {
        SubgroupHeader::new(1, 0, SubgroupId::None, 0, false, false)
    }

    fn object(payload: &'static [u8]) -> BytesMut {
        SubgroupObjectField {
            message_type: header().message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from_static(payload)),
        }
        .encode()
    }

    fn decode_object(decoder: &mut SubgroupDecoder, src: &mut BytesMut) -> Option<Duration> {
        match decoder.decode(src).unwrap()? {
            SubgroupFrame::Object { receive_time, .. } => Some(receive_time),
            SubgroupFrame::Header(_) => panic!("expected an object"),
        }
    }

    #[test]
    fn receive_time_spans_an_object_split_across_reads() {
        // Arrange: the header and the first half of the object arrive together
        let mut decoder = SubgroupDecoder::new();
        let mut src = header().encode();
        let encoded = object(b"keyframe");
        src.extend_from_slice(&encoded[..4]);
        decoder.decode(&mut src).unwrap();
        assert_eq!(decode_object(&mut decoder, &mut src), None);
        std::thread::sleep(GAP);
        src.extend_from_slice(&encoded[4..]);

        // Act
        let receive_time = decode_object(&mut decoder, &mut src);

        // Assert
        assert!(receive_time.unwrap() >= GAP, "{receive_time:?}");
    }

    #[test]
    fn receive_time_of_a_following_object_starts_when_its_first_bytes_arrived() {
        // Arrange: the second object's first bytes arrive with the first object
        let mut decoder = SubgroupDecoder::new();
        let mut src = header().encode();
        decoder.decode(&mut src).unwrap();
        let second = object(b"second");
        src.extend_from_slice(&object(b"first"));
        src.extend_from_slice(&second[..2]);
        decode_object(&mut decoder, &mut src).unwrap();
        std::thread::sleep(GAP);
        assert_eq!(decode_object(&mut decoder, &mut src), None);
        src.extend_from_slice(&second[2..]);

        // Act
        let receive_time = decode_object(&mut decoder, &mut src);

        // Assert
        assert!(receive_time.unwrap() >= GAP, "{receive_time:?}");
    }
}

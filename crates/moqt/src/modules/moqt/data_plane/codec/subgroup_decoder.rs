use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use tokio_util::codec::Decoder;

use super::payload_splitter::{PayloadSplit, PayloadSplitter};
use crate::{
    modules::moqt::data_plane::object::{decode_error::DecodeError, subgroup::SubgroupObjectHead},
    wire::{SubgroupHeader, SubgroupHeaderType, SubgroupObjectField},
};

/// One frame of a subgroup stream as draft-14 encodes it.
#[derive(Debug)]
pub(crate) enum SubgroupFrame {
    Header(SubgroupHeader),
    /// An object whose payload was buffered with its head, or a status object.
    Object {
        field: SubgroupObjectField,
        /// From the first byte of the object being buffered to its last.
        receive_time: Duration,
    },
    /// The head of an object whose payload follows as `Payload` frames.
    ObjectHead(SubgroupObjectHead),
    Payload {
        /// Shares the read buffer.
        data: Bytes,
        /// Set on the last piece of the payload: from the first byte of the
        /// object being buffered to its last.
        completed_in: Option<Duration>,
    },
}

#[derive(Debug)]
pub(crate) struct SubgroupDecoder {
    /// Set once the header is decoded.
    objects: Option<(SubgroupHeaderType, PayloadSplitter)>,
}

impl Decoder for SubgroupDecoder {
    type Item = SubgroupFrame;
    // TODO: define a proper error type.
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some((message_type, splitter)) = &mut self.objects else {
            let mut cursor_buf = std::io::Cursor::new(&src[..]);
            let subgroup = self.decode_header(&mut cursor_buf)?;
            let pos = cursor_buf.position() as usize;
            let _ = src.split_to(pos);
            if let Some(SubgroupFrame::Header(header)) = &subgroup {
                self.objects = Some((header.message_type, PayloadSplitter::after_header(src)));
            }
            return Ok(subgroup);
        };
        let message_type = *message_type;
        let split = splitter.decode(src, |src| {
            Ok::<_, std::io::Error>(Self::decode_head(message_type, src)?.map(|head| {
                let payload_len = head.payload_len();
                (head, payload_len)
            }))
        })?;
        Ok(split.map(|split| match split {
            PayloadSplit::Object {
                head,
                payload,
                receive_time,
            } => SubgroupFrame::Object {
                field: head.with_payload(message_type, payload),
                receive_time,
            },
            PayloadSplit::Head { head, .. } => SubgroupFrame::ObjectHead(head),
            PayloadSplit::Payload { data, completed_in } => {
                SubgroupFrame::Payload { data, completed_in }
            }
        }))
    }

    /// A stream that ends inside an object is an error, also when its head
    /// and part of its payload were already handed out.
    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(frame) => Ok(Some(frame)),
            None if src.is_empty()
                && !matches!(&self.objects, Some((_, splitter)) if splitter.inside_payload()) =>
            {
                Ok(None)
            }
            None => Err(std::io::Error::other("bytes remaining on stream")),
        }
    }
}

impl SubgroupDecoder {
    pub(crate) fn new() -> Self {
        Self { objects: None }
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
        Ok(Some(SubgroupFrame::Header(subgroup_header)))
    }

    fn decode_head(
        message_type: SubgroupHeaderType,
        src: &mut BytesMut,
    ) -> Result<Option<SubgroupObjectHead>, std::io::Error> {
        let mut cursor = std::io::Cursor::new(&src[..]);
        match SubgroupObjectHead::decode(message_type, &mut cursor) {
            Ok(head) => {
                src.advance(cursor.position() as usize);
                Ok(Some(head))
            }
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to decode subgroup object field: {:?}", e),
            )),
        }
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

    fn head_len(object: &BytesMut, payload: &[u8]) -> usize {
        object.len() - payload.len()
    }

    fn decoder_after_header(src: &[u8]) -> (SubgroupDecoder, BytesMut) {
        let mut decoder = SubgroupDecoder::new();
        let mut buf = header().encode();
        buf.extend_from_slice(src);
        assert!(matches!(
            decoder.decode(&mut buf).unwrap(),
            Some(SubgroupFrame::Header(_))
        ));
        (decoder, buf)
    }

    fn decode_all(decoder: &mut SubgroupDecoder, src: &mut BytesMut) -> Vec<SubgroupFrame> {
        std::iter::from_fn(|| decoder.decode(src).unwrap()).collect()
    }

    /// How long the object completed in `frames` took to arrive.
    fn completion_time(frames: &[SubgroupFrame]) -> Option<Duration> {
        frames.iter().find_map(|frame| match frame {
            SubgroupFrame::Object { receive_time, .. } => Some(*receive_time),
            SubgroupFrame::Payload { completed_in, .. } => *completed_in,
            _ => None,
        })
    }

    #[test]
    fn an_object_buffered_with_its_head_is_one_frame() {
        // Arrange
        let (mut decoder, mut src) = decoder_after_header(&object(b"frame"));

        // Act
        let frames = decode_all(&mut decoder, &mut src);

        // Assert
        let [SubgroupFrame::Object { field, .. }] = &frames[..] else {
            panic!("expected one object, got {frames:?}");
        };
        assert_eq!(
            field.subgroup_object,
            SubgroupObject::new_payload(Bytes::from_static(b"frame"))
        );
    }

    #[test]
    fn an_object_split_across_reads_is_its_head_then_its_payload_as_it_arrives() {
        // Arrange: the head and the first two payload bytes arrive first
        let encoded = object(b"keyframe");
        let head_len = head_len(&encoded, b"keyframe");
        let (mut decoder, mut src) = decoder_after_header(&encoded[..head_len + 2]);

        // Act
        let first = decode_all(&mut decoder, &mut src);
        src.extend_from_slice(&encoded[head_len + 2..]);
        let rest = decode_all(&mut decoder, &mut src);

        // Assert
        let [
            SubgroupFrame::ObjectHead(head),
            SubgroupFrame::Payload {
                data: first_data,
                completed_in: None,
            },
        ] = &first[..]
        else {
            panic!("expected the head and a first payload piece, got {first:?}");
        };
        assert_eq!(head.payload_len(), 8);
        assert_eq!(&first_data[..], b"ke");
        let [
            SubgroupFrame::Payload {
                data: rest_data,
                completed_in: Some(_),
            },
        ] = &rest[..]
        else {
            panic!("expected the last payload piece, got {rest:?}");
        };
        assert_eq!(&rest_data[..], b"yframe");
    }

    #[test]
    fn the_object_after_a_split_payload_is_decoded_from_the_same_read() {
        // Arrange: the tail of one payload and the next object arrive together
        let first = object(b"keyframe");
        let head_len = head_len(&first, b"keyframe");
        let (mut decoder, mut src) = decoder_after_header(&first[..head_len]);
        decode_all(&mut decoder, &mut src);
        src.extend_from_slice(&first[head_len..]);
        src.extend_from_slice(&object(b"delta"));

        // Act
        let frames = decode_all(&mut decoder, &mut src);

        // Assert
        let [
            SubgroupFrame::Payload { data, .. },
            SubgroupFrame::Object { field, .. },
        ] = &frames[..]
        else {
            panic!("expected the payload tail and the next object, got {frames:?}");
        };
        assert_eq!(&data[..], b"keyframe");
        assert_eq!(
            field.subgroup_object,
            SubgroupObject::new_payload(Bytes::from_static(b"delta"))
        );
    }

    #[test]
    fn receive_time_spans_an_object_split_across_reads() {
        // Arrange: the header and the first half of the object arrive together
        let encoded = object(b"keyframe");
        let (mut decoder, mut src) = decoder_after_header(&encoded[..4]);
        assert_eq!(completion_time(&decode_all(&mut decoder, &mut src)), None);
        std::thread::sleep(GAP);
        src.extend_from_slice(&encoded[4..]);

        // Act
        let receive_time = completion_time(&decode_all(&mut decoder, &mut src));

        // Assert
        assert!(receive_time.unwrap() >= GAP, "{receive_time:?}");
    }

    #[test]
    fn receive_time_of_a_following_object_starts_when_its_first_bytes_arrived() {
        // Arrange: the second object's first bytes arrive with the first object
        let second = object(b"second");
        let mut buffered = object(b"first");
        buffered.extend_from_slice(&second[..1]);
        let (mut decoder, mut src) = decoder_after_header(&buffered);
        assert!(completion_time(&decode_all(&mut decoder, &mut src)).is_some());
        std::thread::sleep(GAP);
        src.extend_from_slice(&second[1..]);

        // Act
        let receive_time = completion_time(&decode_all(&mut decoder, &mut src));

        // Assert
        assert!(receive_time.unwrap() >= GAP, "{receive_time:?}");
    }

    #[test]
    fn a_stream_ending_inside_a_payload_is_an_error() {
        // Arrange: the head is handed out while the payload never arrives
        let encoded = object(b"keyframe");
        let (mut decoder, mut src) =
            decoder_after_header(&encoded[..head_len(&encoded, b"keyframe")]);
        decode_all(&mut decoder, &mut src);

        // Act
        let result = decoder.decode_eof(&mut src);

        // Assert
        assert!(result.is_err(), "{result:?}");
    }
}

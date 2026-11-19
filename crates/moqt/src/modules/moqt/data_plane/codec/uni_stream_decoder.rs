use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::extensions::buf_get_ext::BufGetExt;

use crate::modules::moqt::data_plane::{
    codec::{
        fetch_decoder::{FetchDecoder, FetchFrame},
        subgroup_decoder::{SubgroupDecoder, SubgroupFrame},
    },
    object::fetch::FetchHeader,
};

#[derive(Debug)]
pub enum UniStreamData {
    Subgroup(SubgroupFrame),
    Fetch(FetchFrame),
}

#[derive(Debug)]
pub(crate) struct UniStreamDecoder {
    inner: InnerDecoder,
}

#[derive(Debug)]
enum InnerDecoder {
    Unknown,
    Subgroup(SubgroupDecoder),
    Fetch(FetchDecoder),
}

impl Decoder for UniStreamDecoder {
    type Item = UniStreamData;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Determine the decoder type from the first message type.
        if let InnerDecoder::Unknown = self.inner {
            let mut cursor = std::io::Cursor::new(&src[..]);
            let message_type = match cursor.try_get_varint() {
                Ok(b) => b,
                Err(_) => return Ok(None),
            };
            if (0x10..=0x1d).contains(&message_type) {
                // 0x10..=0x1d are valid SubgroupHeader message types.
                self.inner = InnerDecoder::Subgroup(SubgroupDecoder::new());
            } else if message_type == FetchHeader::TYPE {
                self.inner = InnerDecoder::Fetch(FetchDecoder::new());
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Unknown message type: {message_type:#x}"),
                ));
            }
        }
        // Decode using the inner decoder chosen above.
        match &mut self.inner {
            InnerDecoder::Subgroup(dec) => match dec.decode(src)? {
                Some(subgroup) => Ok(Some(UniStreamData::Subgroup(subgroup))),
                None => Ok(None),
            },
            InnerDecoder::Fetch(dec) => match dec.decode(src)? {
                Some(item) => Ok(Some(UniStreamData::Fetch(item))),
                None => Ok(None),
            },
            InnerDecoder::Unknown => unreachable!(),
        }
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let InnerDecoder::Subgroup(dec) = &mut self.inner {
            return Ok(dec.decode_eof(src)?.map(UniStreamData::Subgroup));
        }
        match self.decode(src)? {
            Some(item) => Ok(Some(item)),
            None if src.is_empty() => Ok(None),
            None => Err(std::io::Error::other("bytes remaining on stream")),
        }
    }
}

impl UniStreamDecoder {
    pub(crate) fn new() -> Self {
        Self {
            inner: InnerDecoder::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UniStreamDecoder;
    use crate::modules::test_support::{decode_stream, wire_noise};

    #[test]
    fn a_stream_of_any_bytes_yields_no_more_frames_than_bytes() {
        for stream_type in 0..=u8::MAX {
            for mut bytes in wire_noise() {
                // Arrange
                bytes.insert(0, stream_type);

                // Act / Assert: `decode_stream` panics when the frames
                // outnumber the bytes
                decode_stream(UniStreamDecoder::new(), &bytes);
            }
        }
    }

    mod properties {
        use bytes::BytesMut;
        use proptest::prelude::*;

        use super::super::{UniStreamData, UniStreamDecoder};
        use crate::modules::moqt::data_plane::{
            codec::{fetch_decoder::FetchFrame, subgroup_decoder::SubgroupFrame},
            object::{
                extension_headers::ExtensionHeaders,
                fetch::{FetchHeader, FetchObjectField},
                subgroup::{SubgroupHeader, SubgroupObject, SubgroupObjectField},
            },
        };
        use crate::modules::test_support::{cuts, decode_in_chunks, object_strategies};

        /// `SubgroupObjectField` has no `PartialEq`, so its fields are compared.
        fn parts(field: SubgroupObjectField) -> (u64, ExtensionHeaders, SubgroupObject) {
            (
                field.object_id_delta,
                field.extension_headers,
                field.subgroup_object,
            )
        }

        /// The header and objects of a subgroup stream's frames, with each
        /// payload handed out in pieces joined again.
        fn subgroup_objects(
            frames: Vec<UniStreamData>,
        ) -> (SubgroupHeader, Vec<(u64, ExtensionHeaders, SubgroupObject)>) {
            let mut frames = frames.into_iter().map(|frame| match frame {
                UniStreamData::Subgroup(frame) => frame,
                other => panic!("unexpected {other:?}"),
            });
            let Some(SubgroupFrame::Header(header)) = frames.next() else {
                panic!("expected the subgroup header");
            };
            let mut split = None;
            let mut objects = Vec::new();
            for frame in frames {
                match frame {
                    SubgroupFrame::Object { field, .. } => objects.push(parts(field)),
                    SubgroupFrame::ObjectHead(head) => split = Some((head, BytesMut::new())),
                    SubgroupFrame::Payload { data, completed_in } => {
                        let (_, payload) = split.as_mut().expect("a payload follows its head");
                        payload.extend_from_slice(&data);
                        if completed_in.is_some() {
                            let (head, payload) = split.take().unwrap();
                            objects.push(parts(head.with_payload(payload.freeze())));
                        }
                    }
                    SubgroupFrame::Header(_) => panic!("a second subgroup header"),
                }
            }
            (header, objects)
        }

        proptest! {
            #[test]
            fn subgroup_objects_read_back_however_the_stream_is_chunked(
                (header, objects) in object_strategies::draft14_subgroup(),
                cuts in cuts(),
            ) {
                // Arrange
                let mut stream = header.encode().unwrap();
                for object in &objects {
                    stream.extend_from_slice(&object.encode(header.message_type));
                }

                // Act
                let frames = decode_in_chunks(UniStreamDecoder::new(), &stream, &cuts);

                // Assert
                let expected: Vec<_> = objects.into_iter().map(parts).collect();
                prop_assert_eq!(subgroup_objects(frames), (header, expected));
            }

            #[test]
            fn fetch_objects_read_back_however_the_stream_is_chunked(
                objects in object_strategies::draft14_fetch(),
                cuts in cuts(),
            ) {
                // Arrange
                let mut stream = FetchHeader::new(6).encode().unwrap();
                for object in &objects {
                    stream.extend_from_slice(&object.encode().unwrap());
                }

                // Act
                let mut frames = decode_in_chunks(UniStreamDecoder::new(), &stream, &cuts)
                    .into_iter()
                    .map(|frame| match frame {
                        UniStreamData::Fetch(frame) => frame,
                        other => panic!("unexpected {other:?}"),
                    });

                // Assert
                prop_assert!(matches!(frames.next(), Some(FetchFrame::Header(header)) if header == FetchHeader::new(6)));
                let decoded: Vec<FetchObjectField> = frames
                    .map(|frame| match frame {
                        FetchFrame::Object(field) => field,
                        other => panic!("unexpected {other:?}"),
                    })
                    .collect();
                prop_assert_eq!(decoded, objects);
            }
        }
    }
}

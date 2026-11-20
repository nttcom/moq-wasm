//! How draft-14 lays out objects on subgroup streams (§10.4.2), datagrams
//! (§10.3.1) and FETCH responses (§10.4.4).

use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};

#[cfg(test)]
use crate::modules::moqt::data_plane::object::subgroup::SubgroupObjectField;
use crate::modules::{
    moqt::{
        control_plane::control_messages::{
            key_value_pair::KeyValuePair, messages::parameters::location::Location,
        },
        data_plane::{
            codec::{
                fetch_decoder::FetchFrame,
                subgroup_decoder::SubgroupFrame,
                uni_stream_decoder::{UniStreamData, UniStreamDecoder},
            },
            object::{
                canonical_object::{ForwardingPreference, Object, SubgroupStart},
                extension_headers::{ExtensionHeaderType, ExtensionHeaders},
                fetch::{FetchHeader, FetchObjectField},
                object_datagram::ObjectDatagram,
                object_status::ObjectStatus,
                subgroup::{
                    SubgroupHeader, SubgroupHeaderType, SubgroupId, SubgroupObject,
                    SubgroupObjectHead,
                },
                unencodable_object::UnencodableObject,
            },
            stream::{
                fetch_data_receiver::FetchDataReceiver,
                stream_data_receiver::{StreamDataReceiver, SubgroupPart},
                stream_receiver::{StreamReceiveError, UniStreamReceiver},
            },
        },
        version::{self, IncomingStream},
    },
    transport::transport_connection::BoxedReceiveStream,
};

/// Encodes one subgroup stream: its header, then its objects as Object ID
/// Deltas from the previous object.
pub(crate) struct SubgroupEncoder {
    track_alias: u64,
    header: Option<SubgroupHeader>,
    prev_object_id: Option<u64>,
}

impl SubgroupEncoder {
    pub(crate) fn new(track_alias: u64) -> Self {
        Self {
            track_alias,
            header: None,
            prev_object_id: None,
        }
    }

    /// Subgroup 0 omits the Subgroup ID field. Extensions are declared
    /// present, so every object may carry them. Fails with
    /// `UnencodableObject` when draft-14 cannot carry the Group ID or the
    /// Subgroup ID.
    pub(crate) fn header(&mut self, start: SubgroupStart) -> anyhow::Result<BytesMut> {
        check_location(start.group_id, 0)?;
        check_subgroup_id(start.subgroup_id)?;
        let subgroup_id = match start.subgroup_id {
            0 => SubgroupId::None,
            subgroup_id => SubgroupId::Value(subgroup_id),
        };
        self.encode_header(SubgroupHeader::new(
            self.track_alias,
            start.group_id,
            subgroup_id,
            start.publisher_priority,
            true,
            start.ends_group,
        ))
    }

    #[cfg(test)]
    pub(crate) fn raw_header(
        &mut self,
        group_id: u64,
        subgroup_id: SubgroupId,
        publisher_priority: u8,
    ) -> anyhow::Result<BytesMut> {
        self.encode_header(SubgroupHeader::new(
            self.track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
            false,
            false,
        ))
    }

    fn encode_header(&mut self, header: SubgroupHeader) -> anyhow::Result<BytesMut> {
        let bytes = header.encode()?;
        self.header = Some(header);
        Ok(bytes)
    }

    /// Fails when the object id is not greater than the previous object's,
    /// and with `UnencodableObject` when draft-14 cannot carry its Object ID
    /// or extension headers. An object without payload is sent as its status.
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<BytesMut> {
        let mut bytes = self.object_head(object, object.payload.len())?;
        bytes.extend_from_slice(&object.payload);
        Ok(bytes)
    }

    /// The bytes of `object` before a payload of `payload_len` bytes, or its
    /// status when `payload_len` is zero. Fails like `object`.
    pub(crate) fn object_head(
        &mut self,
        object: &Object,
        payload_len: usize,
    ) -> anyhow::Result<BytesMut> {
        let object_id = object.location.object_id;
        check_location(object.location.group_id, object_id)?;
        let object_id_delta = version::object_id_delta(self.prev_object_id, object_id)?;
        let extension_headers = object.extension_headers.clone();
        let head = if payload_len == 0 {
            SubgroupObjectHead::status(
                object_id_delta,
                extension_headers,
                u8::from(object.status) as u64,
            )
        } else {
            SubgroupObjectHead::payload(object_id_delta, extension_headers, payload_len)
        };
        let bytes = head.encode(self.message_type())?;
        self.prev_object_id = Some(object_id);
        Ok(bytes)
    }

    fn message_type(&self) -> SubgroupHeaderType {
        self.header
            .as_ref()
            .expect("the typestate sends the header before any object")
            .message_type
    }

    #[cfg(test)]
    pub(crate) fn raw_object_field(
        &self,
        object_id_delta: u64,
        subgroup_object: SubgroupObject,
    ) -> BytesMut {
        SubgroupObjectField {
            object_id_delta,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object,
        }
        .encode(self.message_type())
    }
}

/// §11.2: Immutable Extensions that hold Immutable Extensions, or a
/// Key-Value-Pair that does not parse, make the track malformed.
pub(crate) fn has_malformed_immutable_extensions(headers: &ExtensionHeaders) -> bool {
    headers
        .immutable_extensions()
        .into_iter()
        .any(|mut contents| {
            while contents.has_remaining() {
                match KeyValuePair::decode(&mut contents) {
                    Ok(pair) if pair.key == ExtensionHeaderType::ImmutableExtensions as u64 => {
                        return true;
                    }
                    Ok(_) => {}
                    Err(_) => return true,
                }
            }
            false
        })
}

/// The first value a draft-14 varint cannot carry (§1.4).
const VARINT_LIMIT: u64 = 1 << 62;

fn check_location(group_id: u64, object_id: u64) -> Result<(), UnencodableObject> {
    if group_id >= VARINT_LIMIT || object_id >= VARINT_LIMIT {
        return Err(UnencodableObject::Location {
            group_id,
            object_id,
        });
    }
    Ok(())
}

fn check_subgroup_id(subgroup_id: u64) -> Result<(), UnencodableObject> {
    if subgroup_id >= VARINT_LIMIT {
        return Err(UnencodableObject::SubgroupId { subgroup_id });
    }
    Ok(())
}

/// Fails with `UnencodableObject` when draft-14 cannot carry the Group ID,
/// Object ID or extension headers.
pub(crate) fn encode_datagram(track_alias: u64, object: &Object) -> anyhow::Result<BytesMut> {
    check_location(object.location.group_id, object.location.object_id)?;
    ObjectDatagram::from_object(track_alias, object).encode()
}

pub(crate) fn encode_fetch_header(request_id: u64) -> anyhow::Result<BytesMut> {
    FetchHeader::new(request_id).encode()
}

/// Fails with `UnencodableObject` when draft-14 cannot carry the object's
/// Group ID, Subgroup ID, Object ID or extension headers.
pub(crate) fn encode_fetch_object(object: &Object) -> anyhow::Result<BytesMut> {
    check_location(object.location.group_id, object.location.object_id)?;
    let field = FetchObjectField::from_object(object);
    check_subgroup_id(field.subgroup_id)?;
    field.encode()
}

pub(crate) fn decode_datagram(data: &mut BytesMut) -> anyhow::Result<(u64, Object)> {
    let datagram = ObjectDatagram::decode(data)?;
    Ok((datagram.track_alias, datagram.into_object()))
}

/// Reads the header that tells a subgroup stream from a FETCH response
/// (§10.4).
pub(crate) async fn accept_uni_stream(
    stream: BoxedReceiveStream,
) -> Result<Option<IncomingStream>, StreamReceiveError> {
    let mut stream = UniStreamReceiver::new(stream, UniStreamDecoder::new());
    match stream.receive().await? {
        None => Ok(None),
        Some(UniStreamData::Subgroup(SubgroupFrame::Header(header))) => {
            Ok(Some(IncomingStream::Subgroup(StreamDataReceiver::new(
                header.track_alias,
                version::SubgroupReader::Draft14(SubgroupReader::new(stream, header)),
            ))))
        }
        Some(UniStreamData::Fetch(FetchFrame::Header(header))) => {
            Ok(Some(IncomingStream::Fetch(FetchDataReceiver::new(
                header.request_id,
                version::FetchReader::Draft14(FetchReader { stream }),
            ))))
        }
        Some(_) => Err(StreamReceiveError::ProtocolViolation(
            "the first message on the stream is not a header".to_string(),
        )),
    }
}

/// Reads the objects of a subgroup stream whose header has been read,
/// resolving Object ID Deltas and the Subgroup ID form of the header.
#[derive(Debug)]
pub(crate) struct SubgroupReader {
    stream: UniStreamReceiver,
    header: SubgroupHeader,
    /// `None` until the first object arrives, for header types that take the
    /// subgroup id from the first object's id.
    subgroup_id: Option<u64>,
    prev_object_id: Option<u64>,
    streaming_payload_len: usize,
}

impl SubgroupReader {
    fn new(stream: UniStreamReceiver, header: SubgroupHeader) -> Self {
        let subgroup_id = match header.subgroup_id {
            SubgroupId::None => Some(0),
            SubgroupId::Value(subgroup_id) => Some(subgroup_id),
            SubgroupId::FirstObjectIdDelta => None,
        };
        Self {
            stream,
            header,
            subgroup_id,
            prev_object_id: None,
            streaming_payload_len: 0,
        }
    }

    pub(crate) fn group_id(&self) -> u64 {
        self.header.group_id
    }

    pub(crate) fn start(&self) -> Option<SubgroupStart> {
        Some(SubgroupStart {
            group_id: self.header.group_id,
            subgroup_id: self.subgroup_id?,
            publisher_priority: self.header.publisher_priority,
            ends_group: self.header.message_type.has_end_of_group(),
        })
    }

    /// Returns objects and the payload parts of objects whose payload had
    /// not arrived with their head; never a header.
    pub(crate) async fn next_part(&mut self) -> Result<Option<SubgroupPart>, StreamReceiveError> {
        let frame = match self.stream.receive().await? {
            Some(UniStreamData::Subgroup(frame)) => frame,
            Some(UniStreamData::Fetch(_)) => {
                unreachable!("Unexpected fetch data in subgroup stream")
            }
            None => {
                tracing::debug!("Stream data ended");
                return Ok(None);
            }
        };
        match frame {
            // The decoder yields the header once, and accept_uni_stream took it.
            SubgroupFrame::Header(_) => unreachable!("a subgroup stream has one header"),
            SubgroupFrame::Object {
                field,
                receive_time,
            } => {
                let object_id = self.object_id(field.resolve_object_id(self.prev_object_id))?;
                let (status, payload) = match field.subgroup_object {
                    SubgroupObject::Payload { data, .. } => (ObjectStatus::Normal, data),
                    SubgroupObject::Status { code, .. } => {
                        (status_from_code(code, object_id)?, Bytes::new())
                    }
                };
                self.log_received(object_id, payload.len(), receive_time);
                let object = self.object(object_id, status, field.extension_headers);
                Ok(Some(SubgroupPart::Object(Object { payload, ..object })))
            }
            SubgroupFrame::ObjectHead(head) => {
                let object_id = self.object_id(head.resolve_object_id(self.prev_object_id))?;
                let payload_len = head.payload_len();
                self.streaming_payload_len = payload_len;
                Ok(Some(SubgroupPart::ObjectStart {
                    object: self.object(object_id, ObjectStatus::Normal, head.extension_headers),
                    payload_len,
                }))
            }
            SubgroupFrame::Payload { data, completed_in } => {
                if let (Some(receive_time), Some(object_id)) = (completed_in, self.prev_object_id) {
                    self.log_received(object_id, self.streaming_payload_len, receive_time);
                }
                Ok(Some(SubgroupPart::Payload {
                    data,
                    last: completed_in.is_some(),
                }))
            }
        }
    }

    /// An Object ID Delta past `u64::MAX` names no Object ID; the draft is
    /// silent, so it fails the stream like an unknown Object Status.
    fn object_id(&self, object_id: Option<u64>) -> Result<u64, StreamReceiveError> {
        object_id.ok_or_else(|| {
            StreamReceiveError::ProtocolViolation(format!(
                "object id delta overflows after object {:?}",
                self.prev_object_id
            ))
        })
    }

    /// The object `object_id` without its payload; it becomes the previous
    /// object for the next Object ID Delta.
    fn object(
        &mut self,
        object_id: u64,
        status: ObjectStatus,
        extension_headers: ExtensionHeaders,
    ) -> Object {
        self.prev_object_id = Some(object_id);
        let subgroup_id = *self.subgroup_id.get_or_insert(object_id);
        Object {
            location: Location {
                group_id: self.header.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id },
            publisher_priority: self.header.publisher_priority,
            status,
            extension_headers,
            payload: Bytes::new(),
            ends_group: false,
        }
    }

    fn log_received(&self, object_id: u64, payload_bytes: usize, receive_time: Duration) {
        tracing::debug!(
            track_alias = self.header.track_alias,
            group_id = self.header.group_id,
            object_id,
            payload_bytes,
            receive_us = receive_time.as_micros() as u64,
            "subgroup object received"
        );
    }
}

/// §10.2.1.1: an Object Status the draft does not define is a protocol
/// violation.
fn status_from_code(code: u64, object_id: u64) -> Result<ObjectStatus, StreamReceiveError> {
    u8::try_from(code)
        .ok()
        .and_then(|code| ObjectStatus::try_from(code).ok())
        .ok_or_else(|| {
            StreamReceiveError::ProtocolViolation(format!(
                "invalid object status {code:#x} on object {object_id}"
            ))
        })
}

/// Reads the objects of a FETCH response whose header has been read.
#[derive(Debug)]
pub(crate) struct FetchReader {
    stream: UniStreamReceiver,
}

impl FetchReader {
    pub(crate) fn stop(&mut self, error_code: u64) {
        self.stream.stop(error_code);
    }

    pub(crate) async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        match self.stream.receive().await? {
            Some(UniStreamData::Fetch(FetchFrame::Object(field))) => Ok(Some(field.into_object())),
            // The decoder yields the header once, and accept_uni_stream took it.
            Some(UniStreamData::Fetch(FetchFrame::Header(_))) => {
                unreachable!("a FETCH stream has one header")
            }
            Some(UniStreamData::Subgroup(_)) => {
                unreachable!("Unexpected subgroup data in fetch stream")
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use std::task::Poll;

    use super::{SubgroupEncoder, SubgroupReader, encode_datagram, encode_fetch_object};
    use crate::modules::{
        moqt::{
            control_plane::control_messages::messages::parameters::location::Location,
            data_plane::{
                codec::uni_stream_decoder::UniStreamDecoder,
                object::{
                    canonical_object::{ForwardingPreference, Object, SubgroupStart},
                    extension_headers::{ExtensionHeaderType, ExtensionHeaders},
                    object_status::ObjectStatus,
                    subgroup::{SubgroupHeader, SubgroupId},
                    unencodable_object::UnencodableObject,
                },
                stream::stream_receiver::{StreamReceiveError, UniStreamReceiver},
            },
        },
        transport::transport_receive_stream::MockTransportReceiveStream,
    };

    fn encoder() -> SubgroupEncoder {
        let mut encoder = SubgroupEncoder::new(1);
        encoder
            .header(SubgroupStart {
                group_id: 0,
                subgroup_id: 0,
                publisher_priority: 0,
                ends_group: false,
            })
            .unwrap();
        encoder
    }

    fn object(object_id: u64, status: ObjectStatus, payload: &'static [u8]) -> Object {
        let mut extension_headers = ExtensionHeaders::default();
        extension_headers.push_immutable_extension(Bytes::from_static(b"ext"));
        Object {
            location: Location {
                group_id: 0,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 0,
            status,
            extension_headers,
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    #[test]
    fn an_object_is_its_delta_extensions_and_length_then_its_payload() {
        // Arrange
        let mut encoder = encoder();
        let object = object(2, ObjectStatus::Normal, b"frame");
        let mut expected = BytesMut::from(&[2u8][..]);
        expected.extend_from_slice(&object.extension_headers.encode().unwrap());
        expected.extend_from_slice(&[5]);
        expected.extend_from_slice(b"frame");

        // Act
        let bytes = encoder.object(&object).unwrap();

        // Assert
        assert_eq!(bytes, expected);
    }

    #[test]
    fn an_object_without_payload_is_a_zero_length_then_its_status() {
        // Arrange
        let mut encoder = encoder();
        encoder
            .object(&object(0, ObjectStatus::Normal, b"frame"))
            .unwrap();
        let object = object(1, ObjectStatus::EndOfGroup, b"");
        let mut expected = BytesMut::from(&[0u8][..]);
        expected.extend_from_slice(&object.extension_headers.encode().unwrap());
        expected.extend_from_slice(&[0, 3]);

        // Act
        let bytes = encoder.object(&object).unwrap();

        // Assert
        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_header_of_group_2_62_fails() {
        // Arrange: a draft-18 Group ID no draft-14 varint carries
        let mut encoder = SubgroupEncoder::new(1);
        let start = SubgroupStart {
            group_id: 1 << 62,
            subgroup_id: 0,
            publisher_priority: 0,
            ends_group: false,
        };

        // Act
        let header = encoder.header(start);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&header.unwrap_err()),
            Some(UnencodableObject::Location {
                group_id: 1 << 62,
                object_id: 0
            })
        );
    }

    #[test]
    fn a_header_of_subgroup_2_62_fails() {
        // Arrange
        let mut encoder = SubgroupEncoder::new(1);
        let start = SubgroupStart {
            group_id: 0,
            subgroup_id: 1 << 62,
            publisher_priority: 0,
            ends_group: false,
        };

        // Act
        let header = encoder.header(start);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&header.unwrap_err()),
            Some(UnencodableObject::SubgroupId {
                subgroup_id: 1 << 62
            })
        );
    }

    #[test]
    fn an_object_of_id_2_62_fails() {
        // Arrange
        let mut encoder = encoder();
        let unencodable = object(1 << 62, ObjectStatus::Normal, b"frame");

        // Act
        let refused = encoder.object(&unencodable);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&refused.unwrap_err()),
            Some(UnencodableObject::Location {
                group_id: 0,
                object_id: 1 << 62
            })
        );
    }

    #[test]
    fn an_object_with_an_extension_value_of_2_62_fails_and_the_next_object_takes_its_place() {
        // Arrange: a draft-18 extension value no draft-14 varint carries
        let mut encoder = encoder();
        let mut unencodable = object(0, ObjectStatus::Normal, b"frame");
        unencodable
            .extension_headers
            .push_prior_group_id_gap(1 << 62);
        let next = object(0, ObjectStatus::Normal, b"frame");

        // Act
        let refused = encoder.object(&unencodable);
        let sent = encoder.object(&next);

        // Assert: the next object is still the subgroup's first, at Object ID Delta 0
        assert_eq!(
            UnencodableObject::in_chain(&refused.unwrap_err()),
            Some(UnencodableObject::ExtensionHeaders {
                key: ExtensionHeaderType::PriorGroupIdGap as u64
            })
        );
        assert_eq!(sent.unwrap()[0], 0);
    }

    #[test]
    fn a_datagram_with_an_extension_value_of_2_62_fails_naming_the_extension() {
        // Arrange
        let mut unencodable = object(0, ObjectStatus::Normal, b"frame");
        unencodable.forwarding = ForwardingPreference::Datagram;
        unencodable
            .extension_headers
            .push_prior_group_id_gap(1 << 62);

        // Act
        let refused = encode_datagram(1, &unencodable);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&refused.unwrap_err()),
            Some(UnencodableObject::ExtensionHeaders {
                key: ExtensionHeaderType::PriorGroupIdGap as u64
            })
        );
    }

    #[test]
    fn a_datagram_of_group_2_62_fails_naming_the_location() {
        // Arrange
        let mut unencodable = object(0, ObjectStatus::Normal, b"frame");
        unencodable.forwarding = ForwardingPreference::Datagram;
        unencodable.location.group_id = 1 << 62;

        // Act
        let refused = encode_datagram(1, &unencodable);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&refused.unwrap_err()),
            Some(UnencodableObject::Location {
                group_id: 1 << 62,
                object_id: 0
            })
        );
    }

    #[test]
    fn a_fetch_object_of_subgroup_2_62_fails_naming_the_subgroup() {
        // Arrange
        let mut unencodable = object(0, ObjectStatus::Normal, b"frame");
        unencodable.forwarding = ForwardingPreference::Subgroup {
            subgroup_id: 1 << 62,
        };

        // Act
        let refused = encode_fetch_object(&unencodable);

        // Assert
        assert_eq!(
            UnencodableObject::in_chain(&refused.unwrap_err()),
            Some(UnencodableObject::SubgroupId {
                subgroup_id: 1 << 62
            })
        );
    }

    #[tokio::test]
    async fn an_object_id_delta_past_u64_max_fails_the_stream() {
        // Arrange: four deltas of 2^62 - 1 reach Object ID u64::MAX, and a
        // fifth object at delta 0 would be u64::MAX + 1
        let header = SubgroupHeader::new(1, 0, SubgroupId::None, 0, false, false);
        let largest_delta = [0xff; 8];
        let mut bytes = header.encode().unwrap().to_vec();
        for _ in 0..4 {
            bytes.extend_from_slice(&largest_delta);
            bytes.extend_from_slice(&[0x01, 0xaa]);
        }
        bytes.extend_from_slice(&[0x00, 0x01, 0xaa]);
        let mut chunks = vec![bytes].into_iter();
        let mut receive_stream = MockTransportReceiveStream::new();
        receive_stream.expect_poll_read().returning(move |_, buf| {
            if let Some(chunk) = chunks.next() {
                buf.put_slice(&chunk);
            }
            Poll::Ready(Ok(()))
        });
        let mut stream = UniStreamReceiver::new(Box::new(receive_stream), UniStreamDecoder::new());
        stream
            .receive()
            .await
            .unwrap()
            .expect("the subgroup header");
        let mut reader = SubgroupReader::new(stream, header);
        for _ in 0..4 {
            reader.next_part().await.unwrap().expect("an object");
        }
        assert_eq!(reader.prev_object_id, Some(u64::MAX));

        // Act
        let result = reader.next_part().await;

        // Assert
        assert!(
            matches!(result, Err(StreamReceiveError::ProtocolViolation(_))),
            "{result:?}"
        );
    }
}

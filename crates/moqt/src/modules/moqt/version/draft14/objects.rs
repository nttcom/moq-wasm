//! How draft-14 lays out objects on subgroup streams (§10.4.2), datagrams
//! (§10.3.1) and FETCH responses (§10.4.4).

use bytes::{Bytes, BytesMut};

use crate::modules::{
    moqt::{
        control_plane::control_messages::messages::parameters::location::Location,
        data_plane::{
            codec::{
                fetch_decoder::FetchFrame,
                subgroup_decoder::SubgroupFrame,
                uni_stream_decoder::{UniStreamData, UniStreamDecoder},
            },
            object::{
                canonical_object::{ForwardingPreference, Object, SubgroupStart},
                extension_headers::ExtensionHeaders,
                fetch::{FetchHeader, FetchObjectField},
                object_datagram::ObjectDatagram,
                object_status::ObjectStatus,
                subgroup::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
            },
            stream::{
                fetch_data_receiver::FetchDataReceiver,
                stream_data_receiver::StreamDataReceiver,
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
    /// present, so every object may carry them.
    pub(crate) fn header(&mut self, start: SubgroupStart) -> BytesMut {
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
    ) -> BytesMut {
        self.encode_header(SubgroupHeader::new(
            self.track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
            false,
            false,
        ))
    }

    fn encode_header(&mut self, header: SubgroupHeader) -> BytesMut {
        let bytes = header.encode();
        self.header = Some(header);
        bytes
    }

    /// Fails when the object id is not greater than the previous object's. An
    /// object without payload is sent as its status.
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<BytesMut> {
        let object_id = object.location.object_id;
        let object_id_delta = match self.prev_object_id {
            None => Some(object_id),
            Some(prev) => prev
                .checked_add(1)
                .and_then(|next| object_id.checked_sub(next)),
        }
        .ok_or_else(|| {
            anyhow::anyhow!(
                "object {object_id} does not follow object {:?} in its subgroup",
                self.prev_object_id
            )
        })?;
        let subgroup_object = if object.payload.is_empty() {
            SubgroupObject::new_status(u8::from(object.status) as u64)
        } else {
            SubgroupObject::new_payload(object.payload.clone())
        };
        let bytes = self
            .raw_object(
                object_id_delta,
                object.extension_headers.clone(),
                subgroup_object,
            )
            .encode();
        self.prev_object_id = Some(object_id);
        Ok(bytes)
    }

    fn raw_object(
        &self,
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        subgroup_object: SubgroupObject,
    ) -> SubgroupObjectField {
        let header = self
            .header
            .as_ref()
            .expect("the typestate sends the header before any object");
        SubgroupObjectField {
            message_type: header.message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_object_field(
        &self,
        object_id_delta: u64,
        subgroup_object: SubgroupObject,
    ) -> BytesMut {
        self.raw_object(
            object_id_delta,
            ExtensionHeaders::default(),
            subgroup_object,
        )
        .encode()
    }
}

pub(crate) fn encode_datagram(track_alias: u64, object: &Object) -> anyhow::Result<BytesMut> {
    ObjectDatagram::from_object(track_alias, object).encode()
}

pub(crate) fn encode_fetch_header(request_id: u64) -> BytesMut {
    FetchHeader::new(request_id).encode()
}

pub(crate) fn encode_fetch_object(object: &Object) -> BytesMut {
    FetchObjectField::from_object(object).encode()
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

    pub(crate) async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        let field = match self.stream.receive().await? {
            Some(UniStreamData::Subgroup(SubgroupFrame::Object(field))) => field,
            // The decoder yields the header once, and accept_uni_stream took it.
            Some(UniStreamData::Subgroup(SubgroupFrame::Header(_))) => {
                unreachable!("a subgroup stream has one header")
            }
            Some(UniStreamData::Fetch(_)) => {
                unreachable!("Unexpected fetch data in subgroup stream")
            }
            None => {
                tracing::debug!("Stream data ended");
                return Ok(None);
            }
        };
        let object_id = field.resolve_object_id(self.prev_object_id);
        self.prev_object_id = Some(object_id);
        let subgroup_id = *self.subgroup_id.get_or_insert(object_id);
        let (status, payload) = match field.subgroup_object {
            SubgroupObject::Payload { data, .. } => (ObjectStatus::Normal, data),
            SubgroupObject::Status { code, .. } => {
                (status_from_code(code, object_id)?, Bytes::new())
            }
        };
        Ok(Some(Object {
            location: Location {
                group_id: self.header.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id },
            publisher_priority: self.header.publisher_priority,
            status,
            extension_headers: field.extension_headers,
            payload,
            ends_group: false,
        }))
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

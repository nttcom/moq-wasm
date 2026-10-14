//! How draft-18 lays out objects on subgroup streams (§11.4.2), datagrams
//! (§11.3.1) and FETCH responses (§11.4.4), and the Padding it discards
//! (§11.5).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::Decoder;

use super::codec::{
    DecodeError, KeyValue, KeyValuePair, get_key_value_pairs, get_vi64, put_key_value_pairs,
    put_vi64,
};
use crate::modules::moqt::{
    control_plane::{
        constants::TerminationErrorCode,
        control_messages::{
            key_value_pair::{KeyValuePair as PropertyPair, VariantType},
            messages::parameters::{group_order::GroupOrder, location::Location},
        },
    },
    data_plane::{
        object::{
            canonical_object::{ForwardingPreference, Object, SubgroupStart},
            extension_headers::ExtensionHeaders,
            object_status::ObjectStatus,
        },
        stream::{
            fetch_data_receiver::FetchDataReceiver,
            stream_data_receiver::StreamDataReceiver,
            stream_receiver::{StreamReceiveError, StreamReceiver},
        },
    },
    version::{self, IncomingStream},
};
use crate::modules::transport::transport_connection::BoxedReceiveStream;

const FETCH_HEADER: u64 = 0x05;
const PADDING_STREAM: u64 = 0x132B_3E28;
const PADDING_DATAGRAM: u64 = 0x132B_3E29;

/// The priority an object inherits when its header omits one (§11.3.1,
/// §11.4.2); the subscription's control message is not visible here, so
/// the default of §12.4 stands in.
const DEFAULT_PUBLISHER_PRIORITY: u8 = 128;

fn protocol_violation(reason: &'static str) -> DecodeError {
    DecodeError::Violation {
        code: TerminationErrorCode::ProtocolViolation,
        reason,
    }
}

fn get_u8(buf: &mut impl Buf) -> Result<u8, DecodeError> {
    buf.try_get_u8().map_err(|_| DecodeError::NeedMoreData)
}

fn take(buf: &mut impl Buf, len: usize) -> Result<Bytes, DecodeError> {
    if buf.remaining() < len {
        return Err(DecodeError::NeedMoreData);
    }
    Ok(buf.copy_to_bytes(len))
}

/// Object Properties (§11.2.1.2).
fn get_properties(buf: &mut impl Buf) -> Result<ExtensionHeaders, DecodeError> {
    let len = get_vi64(buf)? as usize;
    let mut bytes = take(buf, len)?;
    let pairs = get_key_value_pairs(&mut bytes).map_err(|error| match error {
        DecodeError::NeedMoreData => protocol_violation("object properties overrun their length"),
        violation => violation,
    })?;
    Ok(ExtensionHeaders::new(
        pairs
            .into_iter()
            .map(|pair| PropertyPair {
                key: pair.key,
                value: match pair.value {
                    KeyValue::Varint(value) => VariantType::Even(value),
                    KeyValue::Bytes(value) => VariantType::Odd(value),
                },
            })
            .collect(),
    ))
}

/// Sorts the properties by type, as their delta encoding requires.
fn put_properties(buf: &mut BytesMut, properties: &ExtensionHeaders) -> anyhow::Result<()> {
    let mut pairs: Vec<KeyValuePair> = properties
        .key_value_pairs
        .iter()
        .map(|pair| KeyValuePair {
            key: pair.key,
            value: match &pair.value {
                VariantType::Even(value) => KeyValue::Varint(*value),
                VariantType::Odd(value) => KeyValue::Bytes(value.clone()),
            },
        })
        .collect();
    pairs.sort_by_key(|pair| pair.key);
    let mut encoded = BytesMut::new();
    put_key_value_pairs(&mut encoded, &pairs)?;
    put_vi64(buf, encoded.len() as u64);
    buf.put_slice(&encoded);
    Ok(())
}

/// §11.2.1.1: only Normal, End of Group and End of Track exist.
fn status_from_code(code: u64) -> Result<ObjectStatus, DecodeError> {
    match code {
        0x0 => Ok(ObjectStatus::Normal),
        0x3 => Ok(ObjectStatus::EndOfGroup),
        0x4 => Ok(ObjectStatus::EndOfTrack),
        _ => Err(protocol_violation("unknown object status")),
    }
}

fn status_code(status: ObjectStatus) -> anyhow::Result<u64> {
    match status {
        ObjectStatus::Normal => Ok(0x0),
        ObjectStatus::EndOfGroup => Ok(0x3),
        ObjectStatus::EndOfTrack => Ok(0x4),
        ObjectStatus::DoesNotExist => anyhow::bail!("draft-18 has no DoesNotExist status"),
    }
}

const PROPERTIES_BIT: u64 = 0x01;
const SUBGROUP_ID_MODE_MASK: u64 = 0x06;
const SUBGROUP_END_OF_GROUP: u64 = 0x08;
const SUBGROUP_DEFAULT_PRIORITY: u64 = 0x20;
const FIRST_OBJECT: u64 = 0x40;

/// The form 0b0XX1XXXX of §11.4.2.
fn is_subgroup_type(stream_type: u64) -> bool {
    stream_type & !0x7F == 0 && stream_type & 0x10 != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubgroupIdMode {
    Zero,
    FirstObjectId,
    Value(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubgroupHeader {
    pub(crate) track_alias: u64,
    pub(crate) group_id: u64,
    pub(crate) subgroup_id: SubgroupIdMode,
    pub(crate) publisher_priority: Option<u8>,
    pub(crate) properties: bool,
    pub(crate) end_of_group: bool,
    pub(crate) first_object: bool,
}

fn get_subgroup_header(
    stream_type: u64,
    buf: &mut impl Buf,
) -> Result<SubgroupHeader, DecodeError> {
    let track_alias = get_vi64(buf)?;
    let group_id = get_vi64(buf)?;
    let subgroup_id = match (stream_type & SUBGROUP_ID_MODE_MASK) >> 1 {
        0b00 => SubgroupIdMode::Zero,
        0b01 => SubgroupIdMode::FirstObjectId,
        0b10 => SubgroupIdMode::Value(get_vi64(buf)?),
        _ => return Err(protocol_violation("reserved subgroup id mode")),
    };
    let publisher_priority = if stream_type & SUBGROUP_DEFAULT_PRIORITY == 0 {
        Some(get_u8(buf)?)
    } else {
        None
    };
    Ok(SubgroupHeader {
        track_alias,
        group_id,
        subgroup_id,
        publisher_priority,
        properties: stream_type & PROPERTIES_BIT != 0,
        end_of_group: stream_type & SUBGROUP_END_OF_GROUP != 0,
        first_object: stream_type & FIRST_OBJECT != 0,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubgroupObjectField {
    pub(crate) object_id_delta: u64,
    pub(crate) properties: ExtensionHeaders,
    pub(crate) payload: Bytes,
    /// Only for a zero-length object.
    pub(crate) status: Option<u64>,
}

fn get_subgroup_object(
    properties: bool,
    buf: &mut impl Buf,
) -> Result<SubgroupObjectField, DecodeError> {
    let object_id_delta = get_vi64(buf)?;
    let properties = if properties {
        get_properties(buf)?
    } else {
        ExtensionHeaders::default()
    };
    let len = get_vi64(buf)? as usize;
    let (payload, status) = if len == 0 {
        (Bytes::new(), Some(get_vi64(buf)?))
    } else {
        (take(buf, len)?, None)
    };
    Ok(SubgroupObjectField {
        object_id_delta,
        properties,
        payload,
        status,
    })
}

const FETCH_SUBGROUP_MASK: u64 = 0x03;
const FETCH_OBJECT_ID_DELTA: u64 = 0x04;
const FETCH_GROUP_ID_DELTA: u64 = 0x08;
const FETCH_PRIORITY: u64 = 0x10;
const FETCH_PROPERTIES: u64 = 0x20;
const FETCH_DATAGRAM: u64 = 0x40;
const END_OF_NON_EXISTENT_RANGE: u64 = 0x8C;
const END_OF_UNKNOWN_RANGE: u64 = 0x10C;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchField {
    Object {
        flags: u64,
        group_id_delta: Option<u64>,
        subgroup_id: Option<u64>,
        object_id_delta: Option<u64>,
        publisher_priority: Option<u8>,
        properties: ExtensionHeaders,
        payload: Bytes,
    },
    /// §11.4.4.2; `unknown` for 0x10C.
    EndOfRange {
        unknown: bool,
        group_id_delta: u64,
        object_id_delta: u64,
    },
}

fn get_fetch_field(buf: &mut impl Buf) -> Result<FetchField, DecodeError> {
    let flags = get_vi64(buf)?;
    match flags {
        END_OF_NON_EXISTENT_RANGE | END_OF_UNKNOWN_RANGE => {
            return Ok(FetchField::EndOfRange {
                unknown: flags == END_OF_UNKNOWN_RANGE,
                group_id_delta: get_vi64(buf)?,
                object_id_delta: get_vi64(buf)?,
            });
        }
        flags if flags >= 0x80 => return Err(protocol_violation("unknown serialization flags")),
        _ => {}
    }
    let group_id_delta = (flags & FETCH_GROUP_ID_DELTA != 0)
        .then(|| get_vi64(buf))
        .transpose()?;
    let subgroup_id = (flags & FETCH_DATAGRAM == 0 && flags & FETCH_SUBGROUP_MASK == 0x03)
        .then(|| get_vi64(buf))
        .transpose()?;
    let object_id_delta = (flags & FETCH_OBJECT_ID_DELTA != 0)
        .then(|| get_vi64(buf))
        .transpose()?;
    let publisher_priority = (flags & FETCH_PRIORITY != 0)
        .then(|| get_u8(buf))
        .transpose()?;
    let properties = if flags & FETCH_PROPERTIES != 0 {
        get_properties(buf)?
    } else {
        ExtensionHeaders::default()
    };
    let len = get_vi64(buf)? as usize;
    let payload = take(buf, len)?;
    Ok(FetchField::Object {
        flags,
        group_id_delta,
        subgroup_id,
        object_id_delta,
        publisher_priority,
        properties,
        payload,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamFrame {
    SubgroupHeader(SubgroupHeader),
    SubgroupObject(SubgroupObjectField),
    FetchHeader { request_id: u64 },
    FetchField(FetchField),
}

#[derive(Debug, Default)]
enum DecoderState {
    #[default]
    Start,
    Subgroup {
        properties: bool,
    },
    Fetch,
    Padding,
}

/// Decodes a draft-18 unidirectional stream: its header, then the objects
/// the header announces. Padding streams yield nothing and are drained.
#[derive(Debug, Default)]
pub(crate) struct StreamDecoder {
    state: DecoderState,
}

fn invalid_data(error: DecodeError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
}

impl StreamDecoder {
    fn parse(&mut self, buf: &mut &[u8]) -> Result<Option<StreamFrame>, DecodeError> {
        match self.state {
            DecoderState::Start => {
                let stream_type = get_vi64(buf)?;
                if stream_type == PADDING_STREAM {
                    self.state = DecoderState::Padding;
                    return Ok(None);
                }
                if stream_type == FETCH_HEADER {
                    let request_id = get_vi64(buf)?;
                    self.state = DecoderState::Fetch;
                    return Ok(Some(StreamFrame::FetchHeader { request_id }));
                }
                if !is_subgroup_type(stream_type) {
                    return Err(protocol_violation("unknown unidirectional stream type"));
                }
                let header = get_subgroup_header(stream_type, buf)?;
                self.state = DecoderState::Subgroup {
                    properties: header.properties,
                };
                Ok(Some(StreamFrame::SubgroupHeader(header)))
            }
            DecoderState::Subgroup { properties } => Ok(Some(StreamFrame::SubgroupObject(
                get_subgroup_object(properties, buf)?,
            ))),
            DecoderState::Fetch => Ok(Some(StreamFrame::FetchField(get_fetch_field(buf)?))),
            DecoderState::Padding => {
                buf.advance(buf.remaining());
                Ok(None)
            }
        }
    }
}

impl Decoder for StreamDecoder {
    type Item = StreamFrame;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            let mut buf = &src[..];
            let was_padding = matches!(self.state, DecoderState::Padding);
            match self.parse(&mut buf) {
                Ok(frame) => {
                    let consumed = src.len() - buf.len();
                    src.advance(consumed);
                    // A padding stream type is consumed without a frame;
                    // drain what follows it in the same buffer.
                    if frame.is_none() && !was_padding && !src.is_empty() {
                        continue;
                    }
                    return Ok(frame);
                }
                Err(DecodeError::NeedMoreData) => return Ok(None),
                Err(violation) => return Err(invalid_data(violation)),
            }
        }
    }
}

/// Turns the object fields of one subgroup stream into Objects.
#[derive(Debug)]
pub(crate) struct SubgroupState {
    header: SubgroupHeader,
    subgroup_id: Option<u64>,
    prev_object_id: Option<u64>,
}

impl SubgroupState {
    pub(crate) fn new(header: SubgroupHeader) -> Self {
        let subgroup_id = match header.subgroup_id {
            SubgroupIdMode::Zero => Some(0),
            SubgroupIdMode::Value(subgroup_id) => Some(subgroup_id),
            SubgroupIdMode::FirstObjectId => None,
        };
        Self {
            header,
            subgroup_id,
            prev_object_id: None,
        }
    }

    /// `None` until the first object arrives, for a header that takes the
    /// subgroup id from it.
    pub(crate) fn start(&self) -> Option<SubgroupStart> {
        Some(SubgroupStart {
            group_id: self.header.group_id,
            subgroup_id: self.subgroup_id?,
            publisher_priority: self.priority(),
            ends_group: self.header.end_of_group,
        })
    }

    fn priority(&self) -> u8 {
        self.header
            .publisher_priority
            .unwrap_or(DEFAULT_PUBLISHER_PRIORITY)
    }

    pub(crate) fn object(&mut self, field: SubgroupObjectField) -> Result<Object, DecodeError> {
        let object_id = match self.prev_object_id {
            None => field.object_id_delta,
            Some(prev) => prev
                .checked_add(field.object_id_delta)
                .and_then(|id| id.checked_add(1))
                .ok_or(protocol_violation("object id overflows"))?,
        };
        self.prev_object_id = Some(object_id);
        let subgroup_id = *self.subgroup_id.get_or_insert(object_id);
        let status = match field.status {
            Some(code) => status_from_code(code)?,
            None => ObjectStatus::Normal,
        };
        if status != ObjectStatus::Normal && !field.properties.key_value_pairs.is_empty() {
            return Err(protocol_violation(
                "properties on an object that is not Normal",
            ));
        }
        Ok(Object {
            location: Location {
                group_id: self.header.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id },
            publisher_priority: self.priority(),
            status,
            extension_headers: field.properties,
            payload: field.payload,
            ends_group: false,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct PriorLocation {
    group_id: u64,
    object_id: u64,
}

#[derive(Debug, Clone, Copy)]
struct PriorObject {
    subgroup_id: Option<u64>,
    publisher_priority: u8,
}

/// Turns the fields of one FETCH response into Objects. Group ID Deltas
/// follow the FETCH's group order (§11.4.4.1).
#[derive(Debug)]
pub(crate) struct FetchState {
    group_order: GroupOrder,
    location: Option<PriorLocation>,
    object: Option<PriorObject>,
}

impl FetchState {
    pub(crate) fn new() -> Self {
        Self {
            group_order: GroupOrder::Ascending,
            location: None,
            object: None,
        }
    }

    pub(crate) fn set_group_order(&mut self, group_order: GroupOrder) {
        self.group_order = group_order;
    }

    fn resolve_location(
        &self,
        group_id_delta: Option<u64>,
        object_id_delta: Option<u64>,
    ) -> Result<PriorLocation, DecodeError> {
        let Some(prior) = self.location else {
            return match (group_id_delta, object_id_delta) {
                (Some(group_id), Some(object_id)) => Ok(PriorLocation {
                    group_id,
                    object_id,
                }),
                _ => Err(protocol_violation(
                    "first fetch object refers to a prior object",
                )),
            };
        };
        let overflow = || protocol_violation("fetch location overflows");
        match group_id_delta {
            Some(delta) => {
                let step = delta.checked_add(1).ok_or_else(overflow)?;
                let group_id = match self.group_order {
                    GroupOrder::Descending => prior.group_id.checked_sub(step),
                    _ => prior.group_id.checked_add(step),
                }
                .ok_or_else(overflow)?;
                let object_id = match object_id_delta {
                    Some(object_id) => object_id,
                    None => prior.object_id.checked_add(1).ok_or_else(overflow)?,
                };
                Ok(PriorLocation {
                    group_id,
                    object_id,
                })
            }
            None => {
                let object_id = match object_id_delta {
                    Some(delta) => prior.object_id.checked_add(delta),
                    None => prior.object_id.checked_add(1),
                }
                .ok_or_else(overflow)?;
                Ok(PriorLocation {
                    group_id: prior.group_id,
                    object_id,
                })
            }
        }
    }

    /// `Ok(None)` for an End of Range marker, which carries no object.
    pub(crate) fn object(&mut self, field: FetchField) -> Result<Option<Object>, DecodeError> {
        match field {
            FetchField::EndOfRange {
                group_id_delta,
                object_id_delta,
                ..
            } => {
                self.location =
                    Some(self.resolve_location(Some(group_id_delta), Some(object_id_delta))?);
                Ok(None)
            }
            FetchField::Object {
                flags,
                group_id_delta,
                subgroup_id,
                object_id_delta,
                publisher_priority,
                properties,
                payload,
            } => {
                let location = self.resolve_location(group_id_delta, object_id_delta)?;
                let prior = self.object;
                let needs_prior =
                    || protocol_violation("fetch object refers to a missing prior object");
                let forwarding = if flags & FETCH_DATAGRAM != 0 {
                    ForwardingPreference::Datagram
                } else {
                    let prior_subgroup = || {
                        prior
                            .and_then(|prior| prior.subgroup_id)
                            .ok_or_else(needs_prior)
                    };
                    let subgroup_id = match flags & FETCH_SUBGROUP_MASK {
                        0x00 => 0,
                        0x01 => prior_subgroup()?,
                        0x02 => prior_subgroup()?
                            .checked_add(1)
                            .ok_or(protocol_violation("subgroup id overflows"))?,
                        _ => subgroup_id.expect("the decoder reads the field for mode 0x03"),
                    };
                    ForwardingPreference::Subgroup { subgroup_id }
                };
                let publisher_priority = match publisher_priority {
                    Some(priority) => priority,
                    None => prior.ok_or_else(needs_prior)?.publisher_priority,
                };
                self.location = Some(location);
                self.object = Some(PriorObject {
                    subgroup_id: match forwarding {
                        ForwardingPreference::Subgroup { subgroup_id } => Some(subgroup_id),
                        ForwardingPreference::Datagram => prior.and_then(|prior| prior.subgroup_id),
                    },
                    publisher_priority,
                });
                Ok(Some(Object {
                    location: Location {
                        group_id: location.group_id,
                        object_id: location.object_id,
                    },
                    forwarding,
                    publisher_priority,
                    status: ObjectStatus::Normal,
                    extension_headers: properties,
                    payload,
                    ends_group: false,
                }))
            }
        }
    }
}

type UniStreamReceiver = StreamReceiver<{ 64 * 1024 }, StreamDecoder>;

fn violation(error: DecodeError) -> StreamReceiveError {
    StreamReceiveError::ProtocolViolation(error.to_string())
}

/// Reads the header that tells a subgroup stream from a FETCH response
/// (§3.4). `Ok(None)` means the stream finished before its header, or is a
/// padding stream read to its FIN with its bytes discarded (§11.5.1).
pub(crate) async fn accept_uni_stream(
    stream: BoxedReceiveStream,
) -> Result<Option<IncomingStream>, StreamReceiveError> {
    let mut stream = UniStreamReceiver::new(stream, StreamDecoder::default());
    match stream.receive().await? {
        None => Ok(None),
        Some(StreamFrame::SubgroupHeader(header)) => {
            Ok(Some(IncomingStream::Subgroup(StreamDataReceiver::new(
                header.track_alias,
                version::SubgroupReader::Draft18(SubgroupReader {
                    stream,
                    state: SubgroupState::new(header),
                }),
            ))))
        }
        Some(StreamFrame::FetchHeader { request_id }) => {
            Ok(Some(IncomingStream::Fetch(FetchDataReceiver::new(
                request_id,
                version::FetchReader::Draft18(FetchReader {
                    stream,
                    state: FetchState::new(),
                }),
            ))))
        }
        Some(StreamFrame::SubgroupObject(_) | StreamFrame::FetchField(_)) => {
            unreachable!("the decoder yields a header first")
        }
    }
}

#[derive(Debug)]
pub(crate) struct SubgroupReader {
    stream: UniStreamReceiver,
    state: SubgroupState,
}

impl SubgroupReader {
    pub(crate) fn start(&self) -> Option<SubgroupStart> {
        self.state.start()
    }

    pub(crate) async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        match self.stream.receive().await? {
            Some(StreamFrame::SubgroupObject(field)) => {
                Ok(Some(self.state.object(field).map_err(violation)?))
            }
            Some(_) => unreachable!("a subgroup stream has one header"),
            None => Ok(None),
        }
    }
}

#[derive(Debug)]
pub(crate) struct FetchReader {
    stream: UniStreamReceiver,
    state: FetchState,
}

impl FetchReader {
    pub(crate) fn set_group_order(&mut self, group_order: GroupOrder) {
        self.state.set_group_order(group_order);
    }

    /// Skips End of Range markers.
    pub(crate) async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        loop {
            match self.stream.receive().await? {
                Some(StreamFrame::FetchField(field)) => {
                    if let Some(object) = self.state.object(field).map_err(violation)? {
                        return Ok(Some(object));
                    }
                }
                Some(_) => unreachable!("a FETCH stream has one header"),
                None => return Ok(None),
            }
        }
    }
}

pub(crate) struct SubgroupEncoder {
    track_alias: u64,
    prev_object_id: Option<u64>,
}

impl SubgroupEncoder {
    pub(crate) fn new(track_alias: u64) -> Self {
        Self {
            track_alias,
            prev_object_id: None,
        }
    }

    /// Properties are declared present, so every object may carry them.
    pub(crate) fn header(&mut self, start: SubgroupStart) -> BytesMut {
        let mode = match start.subgroup_id {
            0 => 0b00,
            _ => 0b10,
        };
        let mut stream_type = 0x10 | PROPERTIES_BIT | (mode << 1);
        if start.ends_group {
            stream_type |= SUBGROUP_END_OF_GROUP;
        }
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, stream_type);
        put_vi64(&mut buf, self.track_alias);
        put_vi64(&mut buf, start.group_id);
        if start.subgroup_id != 0 {
            put_vi64(&mut buf, start.subgroup_id);
        }
        buf.put_u8(start.publisher_priority);
        buf
    }

    /// Fails when the object id is not greater than the previous object's
    /// or the status does not exist in draft-18.
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<BytesMut> {
        let object_id = object.location.object_id;
        let delta = match self.prev_object_id {
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
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, delta);
        if object.payload.is_empty() && object.status != ObjectStatus::Normal {
            put_properties(&mut buf, &ExtensionHeaders::default())?;
        } else {
            put_properties(&mut buf, &object.extension_headers)?;
        }
        put_vi64(&mut buf, object.payload.len() as u64);
        if object.payload.is_empty() {
            put_vi64(&mut buf, status_code(object.status)?);
        } else {
            buf.put_slice(&object.payload);
        }
        self.prev_object_id = Some(object_id);
        Ok(buf)
    }
}

const DATAGRAM_PROPERTIES: u64 = 0x01;
const DATAGRAM_END_OF_GROUP: u64 = 0x02;
const DATAGRAM_ZERO_OBJECT_ID: u64 = 0x04;
const DATAGRAM_DEFAULT_PRIORITY: u64 = 0x08;
const DATAGRAM_STATUS: u64 = 0x20;

/// Only a Normal object carries properties (§11.2.1.2).
pub(crate) fn encode_datagram(track_alias: u64, object: &Object) -> anyhow::Result<BytesMut> {
    let status = object.payload.is_empty();
    let properties = !object.extension_headers.key_value_pairs.is_empty()
        && object.status == ObjectStatus::Normal;
    let mut datagram_type = 0;
    if properties {
        datagram_type |= DATAGRAM_PROPERTIES;
    }
    if !status && object.ends_group {
        datagram_type |= DATAGRAM_END_OF_GROUP;
    }
    if object.location.object_id == 0 {
        datagram_type |= DATAGRAM_ZERO_OBJECT_ID;
    }
    if status {
        datagram_type |= DATAGRAM_STATUS;
    }
    let mut buf = BytesMut::new();
    put_vi64(&mut buf, datagram_type);
    put_vi64(&mut buf, track_alias);
    put_vi64(&mut buf, object.location.group_id);
    if object.location.object_id != 0 {
        put_vi64(&mut buf, object.location.object_id);
    }
    buf.put_u8(object.publisher_priority);
    if properties {
        put_properties(&mut buf, &object.extension_headers)?;
    }
    if status {
        put_vi64(&mut buf, status_code(object.status)?);
    } else {
        buf.put_slice(&object.payload);
    }
    Ok(buf)
}

/// Returns `(track alias, object)`, or `None` for a padding datagram.
pub(crate) fn decode_datagram(data: &mut impl Buf) -> Result<Option<(u64, Object)>, DecodeError> {
    let datagram_type = get_vi64(data)?;
    if datagram_type == PADDING_DATAGRAM {
        return Ok(None);
    }
    if datagram_type & !0x2F != 0 {
        return Err(protocol_violation("unknown datagram type"));
    }
    if datagram_type & DATAGRAM_STATUS != 0 && datagram_type & DATAGRAM_END_OF_GROUP != 0 {
        return Err(protocol_violation("a status datagram cannot end its group"));
    }
    let track_alias = get_vi64(data)?;
    let group_id = get_vi64(data)?;
    let object_id = if datagram_type & DATAGRAM_ZERO_OBJECT_ID != 0 {
        0
    } else {
        get_vi64(data)?
    };
    let publisher_priority = if datagram_type & DATAGRAM_DEFAULT_PRIORITY != 0 {
        DEFAULT_PUBLISHER_PRIORITY
    } else {
        get_u8(data)?
    };
    let properties = if datagram_type & DATAGRAM_PROPERTIES != 0 {
        let properties = get_properties(data)?;
        if properties.key_value_pairs.is_empty() {
            return Err(protocol_violation(
                "datagram properties bit with no properties",
            ));
        }
        properties
    } else {
        ExtensionHeaders::default()
    };
    let (status, payload) = if datagram_type & DATAGRAM_STATUS != 0 {
        let status = status_from_code(get_vi64(data)?)?;
        if status != ObjectStatus::Normal && !properties.key_value_pairs.is_empty() {
            return Err(protocol_violation(
                "properties on an object that is not Normal",
            ));
        }
        (status, Bytes::new())
    } else {
        (ObjectStatus::Normal, data.copy_to_bytes(data.remaining()))
    };
    Ok(Some((
        track_alias,
        Object {
            location: Location {
                group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority,
            status,
            extension_headers: properties,
            payload,
            ends_group: datagram_type & DATAGRAM_END_OF_GROUP != 0,
        },
    )))
}

pub(crate) fn encode_fetch_header(request_id: u64) -> BytesMut {
    let mut buf = BytesMut::new();
    put_vi64(&mut buf, FETCH_HEADER);
    put_vi64(&mut buf, request_id);
    buf
}

/// The caller writes the objects in the FETCH's group order, which sets the
/// direction of each Group ID Delta (§11.4.4.1).
#[derive(Default)]
pub(crate) struct FetchEncoder {
    location: Option<PriorLocation>,
    object: Option<PriorObject>,
}

impl FetchEncoder {
    /// `Ok(None)` for an object that is not Normal: draft-18 FETCH objects
    /// carry no status (§11.2.1.1).
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<Option<BytesMut>> {
        if object.status != ObjectStatus::Normal {
            return Ok(None);
        }
        let Location {
            group_id,
            object_id,
        } = object.location;
        let mut flags = 0u64;
        let mut fields = BytesMut::new();
        let (group_id_delta, object_id_delta) = match self.location {
            None => (Some(group_id), Some(object_id)),
            Some(prior) if prior.group_id == group_id => {
                let delta = object_id.checked_sub(prior.object_id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "object {object_id} precedes {} in its group",
                        prior.object_id
                    )
                })?;
                (None, (delta != 1).then_some(delta))
            }
            Some(prior) => {
                let distance = group_id.abs_diff(prior.group_id);
                (Some(distance - 1), Some(object_id))
            }
        };
        if let Some(delta) = group_id_delta {
            flags |= FETCH_GROUP_ID_DELTA;
            put_vi64(&mut fields, delta);
        }
        let subgroup_id = match object.forwarding {
            ForwardingPreference::Datagram => {
                flags |= FETCH_DATAGRAM;
                None
            }
            ForwardingPreference::Subgroup { subgroup_id } => {
                let prior_subgroup = self.object.and_then(|prior| prior.subgroup_id);
                if subgroup_id == 0 {
                } else if prior_subgroup == Some(subgroup_id) {
                    flags |= 0x01;
                } else if prior_subgroup.and_then(|prior| prior.checked_add(1)) == Some(subgroup_id)
                {
                    flags |= 0x02;
                } else {
                    flags |= 0x03;
                    put_vi64(&mut fields, subgroup_id);
                }
                Some(subgroup_id)
            }
        };
        if let Some(delta) = object_id_delta {
            flags |= FETCH_OBJECT_ID_DELTA;
            put_vi64(&mut fields, delta);
        }
        if self.object.map(|prior| prior.publisher_priority) != Some(object.publisher_priority) {
            flags |= FETCH_PRIORITY;
            fields.put_u8(object.publisher_priority);
        }
        if !object.extension_headers.key_value_pairs.is_empty() {
            flags |= FETCH_PROPERTIES;
            put_properties(&mut fields, &object.extension_headers)?;
        }
        put_vi64(&mut fields, object.payload.len() as u64);
        fields.put_slice(&object.payload);
        self.location = Some(PriorLocation {
            group_id,
            object_id,
        });
        self.object = Some(PriorObject {
            subgroup_id: subgroup_id.or(self.object.and_then(|prior| prior.subgroup_id)),
            publisher_priority: object.publisher_priority,
        });
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, flags);
        buf.put_slice(&fields);
        Ok(Some(buf))
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::Decoder;

    use super::{
        FetchEncoder, FetchState, PADDING_STREAM, StreamDecoder, StreamFrame, SubgroupEncoder,
        SubgroupState, decode_datagram, encode_datagram, encode_fetch_header,
    };
    use crate::modules::moqt::{
        control_plane::control_messages::{
            key_value_pair::{KeyValuePair, VariantType},
            messages::parameters::{group_order::GroupOrder, location::Location},
        },
        data_plane::object::{
            canonical_object::{ForwardingPreference, Object, SubgroupStart},
            extension_headers::ExtensionHeaders,
            object_status::ObjectStatus,
        },
        version::draft18::codec::{DecodeError, put_vi64},
    };

    fn object(
        group_id: u64,
        object_id: u64,
        forwarding: ForwardingPreference,
        payload: &'static [u8],
    ) -> Object {
        Object {
            location: Location {
                group_id,
                object_id,
            },
            forwarding,
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    fn subgroup(subgroup_id: u64) -> ForwardingPreference {
        ForwardingPreference::Subgroup { subgroup_id }
    }

    fn frames(bytes: BytesMut) -> Vec<StreamFrame> {
        let mut decoder = StreamDecoder::default();
        let mut buf = bytes;
        let mut frames = Vec::new();
        while let Some(frame) = decoder.decode(&mut buf).unwrap() {
            frames.push(frame);
        }
        assert!(buf.is_empty(), "undecoded bytes left: {buf:?}");
        frames
    }

    #[test]
    fn subgroup_objects_round_trip_with_their_properties_and_status() {
        // Arrange
        let mut with_property = object(3, 0, subgroup(2), b"a");
        with_property.extension_headers = ExtensionHeaders::new(vec![
            KeyValuePair {
                key: 7,
                value: VariantType::Odd(Bytes::from_static(b"x")),
            },
            KeyValuePair {
                key: 2,
                value: VariantType::Even(5),
            },
        ]);
        let mut end_of_group = object(3, 4, subgroup(2), b"");
        end_of_group.status = ObjectStatus::EndOfGroup;
        let mut encoder = SubgroupEncoder::new(9);
        let mut stream = encoder.header(SubgroupStart {
            group_id: 3,
            subgroup_id: 2,
            publisher_priority: 128,
            ends_group: true,
        });
        stream.extend_from_slice(&encoder.object(&with_property).unwrap());
        stream.extend_from_slice(&encoder.object(&object(3, 1, subgroup(2), b"b")).unwrap());
        stream.extend_from_slice(&encoder.object(&end_of_group).unwrap());

        // Act
        let mut frames = frames(stream).into_iter();
        let StreamFrame::SubgroupHeader(header) = frames.next().unwrap() else {
            panic!("expected the subgroup header");
        };
        let mut state = SubgroupState::new(header);
        let start = state.start();
        let objects: Vec<Object> = frames
            .map(|frame| match frame {
                StreamFrame::SubgroupObject(field) => state.object(field).unwrap(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();

        // Assert: properties come back in ascending type order
        assert_eq!(state.header.track_alias, 9);
        assert_eq!(
            start,
            Some(SubgroupStart {
                group_id: 3,
                subgroup_id: 2,
                publisher_priority: 128,
                ends_group: true,
            })
        );
        let mut sorted = with_property.clone();
        sorted.extension_headers.key_value_pairs.reverse();
        assert_eq!(
            objects,
            vec![sorted, object(3, 1, subgroup(2), b"b"), end_of_group]
        );
    }

    #[test]
    fn a_reserved_subgroup_id_mode_and_unknown_stream_types_are_rejected() {
        for stream_type in [0x16u64, 0x80, 0x0F] {
            // Arrange
            let mut buf = BytesMut::new();
            put_vi64(&mut buf, stream_type);
            buf.extend_from_slice(&[0, 0, 0, 0]);

            // Act / Assert
            assert!(
                StreamDecoder::default().decode(&mut buf).is_err(),
                "{stream_type:#x}"
            );
        }
    }

    #[test]
    fn a_padding_stream_yields_nothing_and_is_drained() {
        // Arrange
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, PADDING_STREAM);
        buf.extend_from_slice(&[0; 64]);
        let mut decoder = StreamDecoder::default();

        // Act
        let frame = decoder.decode(&mut buf).unwrap();

        // Assert
        assert_eq!(frame, None);
        assert!(buf.is_empty());
    }

    #[test]
    fn datagrams_round_trip_and_padding_datagrams_are_dropped() {
        // Arrange
        let mut ending = object(4, 0, ForwardingPreference::Datagram, b"last");
        ending.ends_group = true;
        let mut status = object(4, 5, ForwardingPreference::Datagram, b"");
        status.status = ObjectStatus::EndOfTrack;
        let mut with_property = object(4, 2, ForwardingPreference::Datagram, b"p");
        with_property.extension_headers = ExtensionHeaders::new(vec![KeyValuePair {
            key: 4,
            value: VariantType::Even(1),
        }]);
        let mut padding = BytesMut::new();
        put_vi64(&mut padding, 0x132B_3E29);

        for sent in [ending, status, with_property] {
            // Act
            let mut encoded = encode_datagram(11, &sent).unwrap().freeze();
            let decoded = decode_datagram(&mut encoded).unwrap();

            // Assert
            assert_eq!(decoded, Some((11, sent)));
        }
        assert_eq!(decode_datagram(&mut padding.freeze()), Ok(None));
    }

    #[test]
    fn invalid_datagram_types_are_protocol_violations() {
        for datagram_type in [0x22u64, 0x10, 0x40] {
            // Arrange
            let mut buf = BytesMut::new();
            put_vi64(&mut buf, datagram_type);
            buf.extend_from_slice(&[0, 0, 0, 0]);

            // Act / Assert
            assert!(
                matches!(
                    decode_datagram(&mut buf.freeze()),
                    Err(DecodeError::Violation { .. })
                ),
                "{datagram_type:#x}"
            );
        }
    }

    fn fetch_round_trip(objects: &[Object], group_order: GroupOrder) -> Vec<Object> {
        let mut encoder = FetchEncoder::default();
        let mut stream = encode_fetch_header(6);
        for object in objects {
            stream.extend_from_slice(&encoder.object(object).unwrap().unwrap());
        }
        let mut frames = frames(stream).into_iter();
        assert_eq!(
            frames.next(),
            Some(StreamFrame::FetchHeader { request_id: 6 })
        );
        let mut state = FetchState::new();
        state.set_group_order(group_order);
        frames
            .filter_map(|frame| match frame {
                StreamFrame::FetchField(field) => state.object(field).unwrap(),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn fetch_objects_round_trip_across_groups_in_either_order() {
        // Arrange
        let mut repriorised = object(1, 2, subgroup(1), b"c");
        repriorised.publisher_priority = 3;
        let ascending = vec![
            object(1, 0, subgroup(0), b"a"),
            object(1, 1, subgroup(1), b"b"),
            repriorised,
            object(1, 5, subgroup(9), b"d"),
            object(4, 0, ForwardingPreference::Datagram, b"e"),
            object(4, 1, subgroup(9), b""),
        ];
        let descending = vec![
            object(9, 0, subgroup(0), b"x"),
            object(7, 3, subgroup(0), b"y"),
            object(2, 0, subgroup(0), b"z"),
        ];

        // Act / Assert
        assert_eq!(
            fetch_round_trip(&ascending, GroupOrder::Ascending),
            ascending
        );
        assert_eq!(
            fetch_round_trip(&descending, GroupOrder::Descending),
            descending
        );
    }

    #[test]
    fn fetch_skips_objects_without_a_draft18_representation() {
        // Arrange
        let mut end_of_group = object(1, 3, subgroup(0), b"");
        end_of_group.status = ObjectStatus::EndOfGroup;

        // Act / Assert
        assert_eq!(FetchEncoder::default().object(&end_of_group).unwrap(), None);
    }

    #[test]
    fn an_end_of_range_marker_moves_the_prior_location() {
        // Arrange: object {1,0}, End of Unknown Range up to {3,4}, then the
        // next object as "prior + 1"
        let mut stream = encode_fetch_header(6);
        stream.extend_from_slice(
            &FetchEncoder::default()
                .object(&object(1, 0, subgroup(0), b"a"))
                .unwrap()
                .unwrap(),
        );
        put_vi64(&mut stream, 0x10C);
        put_vi64(&mut stream, 1);
        put_vi64(&mut stream, 4);
        put_vi64(&mut stream, 0x00);
        put_vi64(&mut stream, 1);
        stream.extend_from_slice(b"b");
        let mut state = FetchState::new();

        // Act
        let objects: Vec<Object> = frames(stream)
            .into_iter()
            .skip(1)
            .filter_map(|frame| match frame {
                StreamFrame::FetchField(field) => state.object(field).unwrap(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();

        // Assert
        assert_eq!(
            objects,
            vec![
                object(1, 0, subgroup(0), b"a"),
                object(3, 5, subgroup(0), b"b")
            ]
        );
    }

    #[test]
    fn a_first_fetch_object_that_refers_to_a_prior_one_is_a_protocol_violation() {
        // Arrange: no Group ID Delta on the first object
        let mut stream = encode_fetch_header(6);
        put_vi64(&mut stream, 0x04 | 0x10);
        put_vi64(&mut stream, 0);
        stream.extend_from_slice(&[128]);
        put_vi64(&mut stream, 0);
        let mut frames = frames(stream).into_iter().skip(1);
        let StreamFrame::FetchField(field) = frames.next().unwrap() else {
            panic!("expected a fetch field");
        };

        // Act / Assert
        assert!(matches!(
            FetchState::new().object(field),
            Err(DecodeError::Violation { .. })
        ));
    }
}

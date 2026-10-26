//! The control messages of draft-18 (§10) and their mapping onto the
//! version-neutral message model (the draft-14 structs). Requests map
//! directly; responses carry no Request ID on the wire (§10.1), so they
//! decode into their draft-18 fields and map to the model with the
//! request's id.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::{
    codec::{
        DecodeError, get_full_track_name, get_key_value_pairs, get_location, get_reason_phrase,
        get_track_namespace, get_u8, get_vi64, put_full_track_name, put_key_value_pairs,
        put_location, put_reason_phrase, put_track_namespace, put_vi64, take,
    },
    error_codes::{
        publish_done_code_from_wire, publish_done_code_to_wire, request_error_code_from_wire,
        request_error_code_to_wire,
    },
    parameters::{
        AUTHORIZATION_TOKEN, EXPIRES, FILL_TIMEOUT, FORWARD, GROUP_ORDER, LARGEST_OBJECT,
        NEW_GROUP_REQUEST, OBJECT_DELIVERY_TIMEOUT, Parameter, RENDEZVOUS_TIMEOUT,
        SUBGROUP_DELIVERY_TIMEOUT, SUBSCRIBER_PRIORITY, SUBSCRIPTION_FILTER,
        TRACK_NAMESPACE_PREFIX, decode_token, encode_token, get_parameters, put_parameters,
    },
};
use crate::modules::moqt::control_plane::{
    control_messages::key_value_pair::{KeyValuePair, VariantType},
    control_messages::messages::{
        fetch::{Fetch, FetchParams},
        fetch_ok::FetchOk,
        parameters::{
            authorization_token::AuthorizationToken, content_exists::ContentExists,
            filter_type::FilterType, group_order::GroupOrder, location::Location,
        },
        publish::Publish,
        publish_namespace::PublishNamespace,
        publish_ok::PublishOk,
        subscribe::Subscribe,
        subscribe_namespace::SubscribeNamespace,
        subscribe_ok::SubscribeOk,
        subscribe_update::SubscribeUpdate,
    },
    error_codes::{PublishDoneCode, RequestErrorCode},
};

pub(crate) const SETUP: u64 = 0x2F00;
pub(crate) const GOAWAY: u64 = 0x10;
pub(crate) const SUBSCRIBE: u64 = 0x3;
pub(crate) const SUBSCRIBE_OK: u64 = 0x4;
pub(crate) const PUBLISH: u64 = 0x1D;
/// Table 5 still lists PUBLISH_OK; the changelog made it a REQUEST_OK
/// alias, so it is only accepted.
const PUBLISH_OK: u64 = 0x1E;
pub(crate) const PUBLISH_DONE: u64 = 0xB;
pub(crate) const FETCH: u64 = 0x16;
pub(crate) const FETCH_OK: u64 = 0x18;
pub(crate) const TRACK_STATUS: u64 = 0xD;
pub(crate) const PUBLISH_NAMESPACE: u64 = 0x6;
pub(crate) const SUBSCRIBE_NAMESPACE: u64 = 0x50;
pub(crate) const SUBSCRIBE_TRACKS: u64 = 0x51;
pub(crate) const NAMESPACE: u64 = 0x8;
pub(crate) const NAMESPACE_DONE: u64 = 0xE;
pub(crate) const PUBLISH_BLOCKED: u64 = 0xF;
pub(crate) const REQUEST_UPDATE: u64 = 0x2;
pub(crate) const REQUEST_OK: u64 = 0x7;
pub(crate) const REQUEST_ERROR: u64 = 0x5;

const SUBSCRIBE_PARAMETERS: &[u64] = &[
    AUTHORIZATION_TOKEN,
    OBJECT_DELIVERY_TIMEOUT,
    SUBGROUP_DELIVERY_TIMEOUT,
    RENDEZVOUS_TIMEOUT,
    SUBSCRIBER_PRIORITY,
    GROUP_ORDER,
    SUBSCRIPTION_FILTER,
    FORWARD,
    NEW_GROUP_REQUEST,
];
const PUBLISH_PARAMETERS: &[u64] = &[AUTHORIZATION_TOKEN, EXPIRES, LARGEST_OBJECT, FORWARD];
const FETCH_PARAMETERS: &[u64] = &[
    AUTHORIZATION_TOKEN,
    SUBSCRIBER_PRIORITY,
    GROUP_ORDER,
    FILL_TIMEOUT,
];
const NAMESPACE_PARAMETERS: &[u64] = &[AUTHORIZATION_TOKEN];
const TRACK_STATUS_PARAMETERS: &[u64] = &[AUTHORIZATION_TOKEN];
const SUBSCRIBE_TRACKS_PARAMETERS: &[u64] = &[AUTHORIZATION_TOKEN, FORWARD];
const REQUEST_UPDATE_PARAMETERS: &[u64] = &[
    AUTHORIZATION_TOKEN,
    OBJECT_DELIVERY_TIMEOUT,
    SUBGROUP_DELIVERY_TIMEOUT,
    SUBSCRIBER_PRIORITY,
    SUBSCRIPTION_FILTER,
    FORWARD,
    NEW_GROUP_REQUEST,
    TRACK_NAMESPACE_PREFIX,
];
const SUBSCRIBE_OK_PARAMETERS: &[u64] = &[EXPIRES, LARGEST_OBJECT];
/// The union of PUBLISH_OK, REQUEST_UPDATE_OK and TRACK_STATUS_OK.
const REQUEST_OK_PARAMETERS: &[u64] = &[
    OBJECT_DELIVERY_TIMEOUT,
    SUBGROUP_DELIVERY_TIMEOUT,
    SUBSCRIBER_PRIORITY,
    GROUP_ORDER,
    SUBSCRIPTION_FILTER,
    EXPIRES,
    LARGEST_OBJECT,
    FORWARD,
    NEW_GROUP_REQUEST,
];

// Track Property types (§15.8) the model maps.
const OBJECT_DELIVERY_TIMEOUT_PROPERTY: u64 = 0x02;
const MAX_CACHE_DURATION_PROPERTY: u64 = 0x04;
const DEFAULT_PUBLISHER_PRIORITY_PROPERTY: u64 = 0x0E;
const IMMUTABLE_PROPERTIES: u64 = 0x0B;
const DEFAULT_PUBLISHER_GROUP_ORDER_PROPERTY: u64 = 0x22;
/// §12.4: the priority of a track whose control message omits DEFAULT
/// PUBLISHER PRIORITY.
pub(crate) const DEFAULT_PUBLISHER_PRIORITY: u8 = 128;
/// §2.5.1.
const MANDATORY_TRACK_PROPERTIES: std::ops::RangeInclusive<u64> = 0x4000..=0x7FFF;

const MAX_URI_LENGTH: u64 = 8192;

/// `Type (vi64), Length (16), Payload`. Fails when the payload exceeds the
/// 16-bit length.
pub(crate) fn frame(message_type: u64, payload: &[u8]) -> anyhow::Result<BytesMut> {
    let length = u16::try_from(payload.len())
        .map_err(|_| anyhow::anyhow!("control message longer than 2^16-1 bytes"))?;
    let mut buf = BytesMut::new();
    put_vi64(&mut buf, message_type);
    buf.put_u16(length);
    buf.put_slice(payload);
    Ok(buf)
}

/// Splits one framed message off `buf`; `NeedMoreData` leaves `buf`
/// untouched.
pub(crate) fn take_message(buf: &mut BytesMut) -> Result<(u64, Bytes), DecodeError> {
    let mut peek = &buf[..];
    let message_type = get_vi64(&mut peek)?;
    if peek.remaining() < 2 {
        return Err(DecodeError::NeedMoreData);
    }
    let length = peek.get_u16() as usize;
    if peek.remaining() < length {
        return Err(DecodeError::NeedMoreData);
    }
    let header = buf.len() - peek.len();
    buf.advance(header);
    Ok((message_type, buf.split_to(length).freeze()))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SetupMessage {
    pub(crate) path: Option<String>,
    pub(crate) authority: Option<String>,
    pub(crate) authorization_tokens: Vec<AuthorizationToken>,
    pub(crate) max_auth_token_cache_size: u64,
    pub(crate) implementation: Option<String>,
}

const PATH_OPTION: u64 = 0x01;
const AUTHORIZATION_TOKEN_OPTION: u64 = 0x03;
const MAX_AUTH_TOKEN_CACHE_SIZE_OPTION: u64 = 0x04;
const AUTHORITY_OPTION: u64 = 0x05;
const MOQT_IMPLEMENTATION_OPTION: u64 = 0x07;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GoAwayMessage {
    pub(crate) new_session_uri: String,
    /// Only on the control stream.
    pub(crate) request_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubscribeOkMessage {
    pub(crate) track_alias: u64,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) track_properties: Vec<KeyValuePair>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FetchOkMessage {
    pub(crate) end_of_track: bool,
    pub(crate) end_location: Location,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) track_properties: Vec<KeyValuePair>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestOkMessage {
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) track_properties: Vec<KeyValuePair>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestErrorMessage {
    pub(crate) code: RequestErrorCode,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PublishDoneMessage {
    pub(crate) code: PublishDoneCode,
    pub(crate) stream_count: u64,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestUpdateMessage {
    pub(crate) request_id: u64,
    pub(crate) parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubscribeTracksMessage {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_prefix: Vec<String>,
    pub(crate) authorization_tokens: Vec<AuthorizationToken>,
    /// The Forward State of the PUBLISH messages it leads to (§10.19).
    pub(crate) forward: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Message {
    Setup(SetupMessage),
    GoAway(GoAwayMessage),
    Subscribe(Subscribe),
    SubscribeOk(SubscribeOkMessage),
    RequestUpdate(RequestUpdateMessage),
    /// The Track Properties stay alongside for the Mandatory Track
    /// Property check of §2.5.1.
    Publish {
        publish: Publish,
        track_properties: Vec<KeyValuePair>,
    },
    PublishDone(PublishDoneMessage),
    Fetch(Fetch),
    FetchOk(FetchOkMessage),
    TrackStatus(Subscribe),
    PublishNamespace(PublishNamespace),
    Namespace(Vec<String>),
    NamespaceDone(Vec<String>),
    SubscribeNamespace(SubscribeNamespace),
    SubscribeTracks(SubscribeTracksMessage),
    PublishBlocked,
    RequestOk(RequestOkMessage),
    RequestError(RequestErrorMessage),
}

/// An unknown type or a payload whose length does not match the message
/// closes the session with PROTOCOL_VIOLATION (§10).
pub(crate) fn decode_message(message_type: u64, payload: Bytes) -> Result<Message, DecodeError> {
    let mut buf = payload;
    // The framing delivered the whole payload, so running short is a length
    // mismatch rather than a reason to wait.
    let message = decode_payload(message_type, &mut buf).map_err(|error| match error {
        DecodeError::NeedMoreData => {
            DecodeError::protocol_violation("control message length does not match")
        }
        violation => violation,
    })?;
    if buf.has_remaining() {
        return Err(DecodeError::protocol_violation(
            "control message length does not match",
        ));
    }
    Ok(message)
}

fn decode_payload(message_type: u64, buf: &mut Bytes) -> Result<Message, DecodeError> {
    Ok(match message_type {
        SETUP => Message::Setup(decode_setup(buf)?),
        GOAWAY => Message::GoAway(decode_go_away(buf)?),
        SUBSCRIBE => Message::Subscribe(decode_subscribe(buf, SUBSCRIBE_PARAMETERS)?),
        TRACK_STATUS => Message::TrackStatus(decode_subscribe(buf, TRACK_STATUS_PARAMETERS)?),
        SUBSCRIBE_OK => Message::SubscribeOk(SubscribeOkMessage {
            track_alias: get_vi64(buf)?,
            parameters: get_parameters(buf, SUBSCRIBE_OK_PARAMETERS)?,
            track_properties: get_track_properties(buf)?,
        }),
        REQUEST_UPDATE => Message::RequestUpdate(RequestUpdateMessage {
            request_id: get_vi64(buf)?,
            parameters: get_parameters(buf, REQUEST_UPDATE_PARAMETERS)?,
        }),
        PUBLISH => {
            let (publish, track_properties) = decode_publish(buf)?;
            Message::Publish {
                publish,
                track_properties,
            }
        }
        PUBLISH_DONE => Message::PublishDone(PublishDoneMessage {
            code: publish_done_code_from_wire(get_vi64(buf)?),
            stream_count: get_vi64(buf)?,
            reason: get_reason_phrase(buf)?,
        }),
        FETCH => Message::Fetch(decode_fetch(buf)?),
        FETCH_OK => Message::FetchOk(FetchOkMessage {
            end_of_track: match get_u8(buf)? {
                0 => false,
                1 => true,
                _ => {
                    return Err(DecodeError::protocol_violation(
                        "End Of Track other than 0 or 1",
                    ));
                }
            },
            end_location: get_location(buf)?,
            parameters: get_parameters(buf, &[])?,
            track_properties: get_track_properties(buf)?,
        }),
        PUBLISH_NAMESPACE => {
            let request_id = get_vi64(buf)?;
            let track_namespace = get_track_namespace(buf)?;
            let tokens = tokens(get_parameters(buf, NAMESPACE_PARAMETERS)?);
            Message::PublishNamespace(PublishNamespace {
                request_id,
                track_namespace,
                authorization_token: tokens,
            })
        }
        SUBSCRIBE_NAMESPACE => {
            let request_id = get_vi64(buf)?;
            let track_namespace_prefix = get_track_namespace(buf)?;
            let tokens = tokens(get_parameters(buf, NAMESPACE_PARAMETERS)?);
            Message::SubscribeNamespace(SubscribeNamespace {
                request_id,
                track_namespace_prefix,
                authorization_token: tokens,
            })
        }
        SUBSCRIBE_TRACKS => {
            let request_id = get_vi64(buf)?;
            let track_namespace_prefix = get_track_namespace(buf)?;
            let parameters = get_parameters(buf, SUBSCRIBE_TRACKS_PARAMETERS)?;
            // §10.2.12: an omitted FORWARD means 1.
            let forward = !parameters
                .iter()
                .any(|parameter| *parameter == Parameter::Forward(false));
            Message::SubscribeTracks(SubscribeTracksMessage {
                request_id,
                track_namespace_prefix,
                authorization_tokens: tokens(parameters),
                forward,
            })
        }
        NAMESPACE => Message::Namespace(get_track_namespace(buf)?),
        NAMESPACE_DONE => Message::NamespaceDone(get_track_namespace(buf)?),
        PUBLISH_BLOCKED => {
            get_full_track_name(buf)?;
            Message::PublishBlocked
        }
        REQUEST_OK | PUBLISH_OK => Message::RequestOk(RequestOkMessage {
            parameters: get_parameters(buf, REQUEST_OK_PARAMETERS)?,
            track_properties: get_track_properties(buf)?,
        }),
        REQUEST_ERROR => Message::RequestError(decode_request_error(buf)?),
        _ => {
            return Err(DecodeError::protocol_violation(
                "unknown control message type",
            ));
        }
    })
}

fn tokens(parameters: Vec<Parameter>) -> Vec<AuthorizationToken> {
    parameters
        .into_iter()
        .filter_map(|parameter| match parameter {
            Parameter::AuthorizationToken(token) => Some(token),
            _ => None,
        })
        .collect()
}

fn token_parameters(tokens: &[AuthorizationToken]) -> Vec<Parameter> {
    tokens
        .iter()
        .cloned()
        .map(Parameter::AuthorizationToken)
        .collect()
}

fn get_uri(buf: &mut Bytes) -> Result<String, DecodeError> {
    let len = get_vi64(buf)?;
    if len > MAX_URI_LENGTH {
        return Err(DecodeError::protocol_violation(
            "URI longer than 8192 bytes",
        ));
    }
    String::from_utf8(take(buf, len as usize)?.to_vec())
        .map_err(|_| DecodeError::protocol_violation("string is not UTF-8"))
}

fn put_string(buf: &mut BytesMut, value: &str) {
    put_vi64(buf, value.len() as u64);
    buf.put_slice(value.as_bytes());
}

fn decode_setup(buf: &mut Bytes) -> Result<SetupMessage, DecodeError> {
    let mut setup = SetupMessage::default();
    // §10.3: unknown Setup Options are ignored.
    for option in get_key_value_pairs(buf)? {
        match (option.key, option.value) {
            (PATH_OPTION, VariantType::Odd(value)) => setup.path = utf8_option(value)?,
            (AUTHORITY_OPTION, VariantType::Odd(value)) => setup.authority = utf8_option(value)?,
            (MOQT_IMPLEMENTATION_OPTION, VariantType::Odd(value)) => {
                setup.implementation = Some(String::from_utf8_lossy(&value).into_owned())
            }
            (MAX_AUTH_TOKEN_CACHE_SIZE_OPTION, VariantType::Even(value)) => {
                setup.max_auth_token_cache_size = value
            }
            (AUTHORIZATION_TOKEN_OPTION, VariantType::Odd(value)) => {
                setup.authorization_tokens.push(decode_token(value)?)
            }
            _ => {}
        }
    }
    Ok(setup)
}

fn utf8_option(value: Bytes) -> Result<Option<String>, DecodeError> {
    String::from_utf8(value.to_vec())
        .map(Some)
        .map_err(|_| DecodeError::protocol_violation("setup option is not UTF-8"))
}

pub(crate) fn encode_setup(setup: &SetupMessage) -> anyhow::Result<BytesMut> {
    let mut options = Vec::new();
    let bytes_option = |key, value: &str| KeyValuePair {
        key,
        value: VariantType::Odd(Bytes::copy_from_slice(value.as_bytes())),
    };
    if let Some(path) = &setup.path {
        options.push(bytes_option(PATH_OPTION, path));
    }
    for token in &setup.authorization_tokens {
        options.push(KeyValuePair {
            key: AUTHORIZATION_TOKEN_OPTION,
            value: VariantType::Odd(encode_token(token)?.freeze()),
        });
    }
    if setup.max_auth_token_cache_size != 0 {
        options.push(KeyValuePair {
            key: MAX_AUTH_TOKEN_CACHE_SIZE_OPTION,
            value: VariantType::Even(setup.max_auth_token_cache_size),
        });
    }
    if let Some(authority) = &setup.authority {
        options.push(bytes_option(AUTHORITY_OPTION, authority));
    }
    if let Some(implementation) = &setup.implementation {
        options.push(bytes_option(MOQT_IMPLEMENTATION_OPTION, implementation));
    }
    let mut payload = BytesMut::new();
    put_key_value_pairs(&mut payload, &options)?;
    frame(SETUP, &payload)
}

fn decode_go_away(buf: &mut Bytes) -> Result<GoAwayMessage, DecodeError> {
    let new_session_uri = get_uri(buf)?;
    // Timeout: a hint the session does not act on.
    get_vi64(buf)?;
    let request_id = buf.has_remaining().then(|| get_vi64(buf)).transpose()?;
    Ok(GoAwayMessage {
        new_session_uri,
        request_id,
    })
}

pub(crate) fn encode_go_away(go_away: &GoAwayMessage) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_string(&mut payload, &go_away.new_session_uri);
    // Timeout 0: no specific timeout (§10.4).
    put_vi64(&mut payload, 0);
    if let Some(request_id) = go_away.request_id {
        put_vi64(&mut payload, request_id);
    }
    frame(GOAWAY, &payload)
}

/// An absent filter is an unfiltered subscription, which AbsoluteStart
/// {0, 0} also means (§5.1.2).
const UNFILTERED: FilterType = FilterType::AbsoluteStart {
    location: Location {
        group_id: 0,
        object_id: 0,
    },
};
const DEFAULT_SUBSCRIBER_PRIORITY: u8 = 128;

/// SUBSCRIBE and TRACK_STATUS share a format (§10.14); `allowed` lists the
/// parameters of the one being read.
fn decode_subscribe(buf: &mut Bytes, allowed: &[u64]) -> Result<Subscribe, DecodeError> {
    let request_id = get_vi64(buf)?;
    let (track_namespace, track_name) = get_full_track_name(buf)?;
    let mut subscribe = Subscribe {
        request_id,
        track_namespace,
        track_name,
        subscriber_priority: DEFAULT_SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Publisher,
        forward: true,
        filter_type: UNFILTERED,
        authorization_tokens: vec![],
        delivery_timeout: None,
    };
    for parameter in get_parameters(buf, allowed)? {
        match parameter {
            Parameter::SubscriberPriority(priority) => subscribe.subscriber_priority = priority,
            Parameter::GroupOrder(order) => subscribe.group_order = order,
            Parameter::Forward(forward) => subscribe.forward = forward,
            Parameter::SubscriptionFilter(filter) => subscribe.filter_type = filter,
            Parameter::AuthorizationToken(token) => subscribe.authorization_tokens.push(token),
            Parameter::ObjectDeliveryTimeout(timeout) => subscribe.delivery_timeout = Some(timeout),
            _ => {}
        }
    }
    Ok(subscribe)
}

fn subscribe_parameters(subscribe: &Subscribe) -> Vec<Parameter> {
    let mut parameters = vec![
        Parameter::SubscriberPriority(subscribe.subscriber_priority),
        Parameter::Forward(subscribe.forward),
        Parameter::SubscriptionFilter(subscribe.filter_type),
    ];
    if subscribe.group_order != GroupOrder::Publisher {
        parameters.push(Parameter::GroupOrder(subscribe.group_order));
    }
    if let Some(timeout) = subscribe.delivery_timeout {
        parameters.push(Parameter::ObjectDeliveryTimeout(timeout));
    }
    parameters.extend(token_parameters(&subscribe.authorization_tokens));
    parameters
}

fn encode_full_track_name_request(
    message_type: u64,
    request_id: u64,
    namespace: &[String],
    name: &str,
    parameters: &[Parameter],
) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, request_id);
    put_full_track_name(&mut payload, namespace, name);
    put_parameters(&mut payload, parameters)?;
    frame(message_type, &payload)
}

pub(crate) fn encode_subscribe(subscribe: &Subscribe) -> anyhow::Result<BytesMut> {
    encode_full_track_name_request(
        SUBSCRIBE,
        subscribe.request_id,
        &subscribe.track_namespace,
        &subscribe.track_name,
        &subscribe_parameters(subscribe),
    )
}

/// §10.14: TRACK_STATUS carries no delivery parameters.
pub(crate) fn encode_track_status(track_status: &Subscribe) -> anyhow::Result<BytesMut> {
    encode_full_track_name_request(
        TRACK_STATUS,
        track_status.request_id,
        &track_status.track_namespace,
        &track_status.track_name,
        &token_parameters(&track_status.authorization_tokens),
    )
}

fn varint_property(key: u64, value: u64) -> KeyValuePair {
    KeyValuePair {
        key,
        value: VariantType::Even(value),
    }
}

/// The model's track-level fields as Track Properties, in ascending type
/// order.
fn track_properties(
    delivery_timeout: Option<u64>,
    max_cache_duration: Option<u64>,
    group_order: GroupOrder,
) -> Vec<KeyValuePair> {
    let mut properties = Vec::new();
    if let Some(timeout) = delivery_timeout {
        properties.push(varint_property(OBJECT_DELIVERY_TIMEOUT_PROPERTY, timeout));
    }
    if let Some(duration) = max_cache_duration {
        properties.push(varint_property(MAX_CACHE_DURATION_PROPERTY, duration));
    }
    properties.extend(
        match group_order {
            GroupOrder::Ascending => Some(0x1),
            GroupOrder::Descending => Some(0x2),
            GroupOrder::Publisher => None,
        }
        .map(|order| varint_property(DEFAULT_PUBLISHER_GROUP_ORDER_PROPERTY, order)),
    );
    properties
}

/// §12.7: a property is searched in the mutable list, then inside
/// Immutable Properties.
fn searched_properties(properties: &[KeyValuePair]) -> Vec<KeyValuePair> {
    let mut searched = properties.to_vec();
    for property in properties {
        if let KeyValuePair {
            key: IMMUTABLE_PROPERTIES,
            value: VariantType::Odd(contents),
        } = property
            && let Ok(immutable) = get_key_value_pairs(&mut contents.clone())
        {
            searched.extend(immutable);
        }
    }
    searched
}

fn varint_property_value(properties: &[KeyValuePair], key: u64) -> Option<u64> {
    searched_properties(properties)
        .into_iter()
        .find_map(|property| match property {
            KeyValuePair {
                key: found,
                value: VariantType::Even(value),
            } if found == key => Some(value),
            _ => None,
        })
}

/// The Track Properties ending a message (§2.5). Priorities above 255
/// (§12.4) and group orders other than Ascending or Descending (§12.5), in
/// the mutable list or inside Immutable Properties, close the session with
/// PROTOCOL_VIOLATION.
fn get_track_properties(buf: &mut Bytes) -> Result<Vec<KeyValuePair>, DecodeError> {
    let properties = get_key_value_pairs(buf)?;
    for property in searched_properties(&properties) {
        let KeyValuePair {
            key,
            value: VariantType::Even(value),
        } = property
        else {
            continue;
        };
        match key {
            DEFAULT_PUBLISHER_PRIORITY_PROPERTY if value > u64::from(u8::MAX) => {
                return Err(DecodeError::protocol_violation(
                    "DEFAULT PUBLISHER PRIORITY above 255",
                ));
            }
            DEFAULT_PUBLISHER_GROUP_ORDER_PROPERTY if !matches!(value, 0x1 | 0x2) => {
                return Err(DecodeError::protocol_violation(
                    "DEFAULT PUBLISHER GROUP ORDER is neither Ascending nor Descending",
                ));
            }
            _ => {}
        }
    }
    Ok(properties)
}

/// §12.4.
pub(crate) fn default_publisher_priority(properties: &[KeyValuePair]) -> u8 {
    varint_property_value(properties, DEFAULT_PUBLISHER_PRIORITY_PROPERTY)
        .and_then(|priority| u8::try_from(priority).ok())
        .unwrap_or(DEFAULT_PUBLISHER_PRIORITY)
}

fn property_group_order(properties: &[KeyValuePair]) -> GroupOrder {
    match varint_property_value(properties, DEFAULT_PUBLISHER_GROUP_ORDER_PROPERTY) {
        Some(0x2) => GroupOrder::Descending,
        _ => GroupOrder::Ascending,
    }
}

/// §12.7: Immutable Properties that do not parse, or that hold Immutable
/// Properties, make the track malformed.
pub(crate) fn has_malformed_immutable_properties(properties: &[KeyValuePair]) -> bool {
    properties.iter().any(|property| match property {
        KeyValuePair {
            key: IMMUTABLE_PROPERTIES,
            value: VariantType::Odd(contents),
        } => get_key_value_pairs(&mut contents.clone()).map_or(true, |immutable| {
            immutable
                .iter()
                .any(|property| property.key == IMMUTABLE_PROPERTIES)
        }),
        _ => false,
    })
}

/// §2.5.1: none is understood, so any Mandatory Track Property means the
/// track must not be processed.
pub(crate) fn has_mandatory_track_property(properties: &[KeyValuePair]) -> bool {
    searched_properties(properties)
        .iter()
        .any(|property| MANDATORY_TRACK_PROPERTIES.contains(&property.key))
}

fn content_exists(parameters: &[Parameter]) -> ContentExists {
    parameters
        .iter()
        .find_map(|parameter| match parameter {
            Parameter::LargestObject(location) => Some(ContentExists::True {
                location: *location,
            }),
            _ => None,
        })
        .unwrap_or(ContentExists::False)
}

fn largest_object(content_exists: &ContentExists) -> Option<Parameter> {
    match content_exists {
        ContentExists::True { location } => Some(Parameter::LargestObject(*location)),
        ContentExists::False => None,
    }
}

fn decode_publish(buf: &mut Bytes) -> Result<(Publish, Vec<KeyValuePair>), DecodeError> {
    let request_id = get_vi64(buf)?;
    let (track_namespace_tuple, track_name) = get_full_track_name(buf)?;
    let track_alias = get_vi64(buf)?;
    let parameters = get_parameters(buf, PUBLISH_PARAMETERS)?;
    let properties = get_track_properties(buf)?;
    let forward = parameters
        .iter()
        .find_map(|parameter| match parameter {
            Parameter::Forward(forward) => Some(*forward),
            _ => None,
        })
        .unwrap_or(true);
    let publish = Publish {
        request_id,
        track_namespace_tuple,
        track_name,
        track_alias,
        group_order: property_group_order(&properties),
        content_exists: content_exists(&parameters),
        forward,
        authorization_tokens: tokens(parameters),
        delivery_timeout: varint_property_value(&properties, OBJECT_DELIVERY_TIMEOUT_PROPERTY),
        max_duration: varint_property_value(&properties, MAX_CACHE_DURATION_PROPERTY),
    };
    Ok((publish, properties))
}

pub(crate) fn encode_publish(publish: &Publish) -> anyhow::Result<BytesMut> {
    let mut parameters = vec![Parameter::Forward(publish.forward)];
    parameters.extend(largest_object(&publish.content_exists));
    parameters.extend(token_parameters(&publish.authorization_tokens));
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, publish.request_id);
    put_full_track_name(
        &mut payload,
        &publish.track_namespace_tuple,
        &publish.track_name,
    );
    put_vi64(&mut payload, publish.track_alias);
    put_parameters(&mut payload, &parameters)?;
    put_key_value_pairs(
        &mut payload,
        &track_properties(
            publish.delivery_timeout,
            publish.max_duration,
            publish.group_order,
        ),
    )?;
    frame(PUBLISH, &payload)
}

const STANDALONE: u64 = 0x1;
const RELATIVE_JOINING: u64 = 0x2;
const ABSOLUTE_JOINING: u64 = 0x3;

fn decode_fetch(buf: &mut Bytes) -> Result<Fetch, DecodeError> {
    let request_id = get_vi64(buf)?;
    let fetch_params = match get_vi64(buf)? {
        STANDALONE => {
            let (track_namespace, track_name) = get_full_track_name(buf)?;
            FetchParams::Standalone {
                track_namespace,
                track_name,
                start_location: get_location(buf)?,
                end_location: get_location(buf)?,
            }
        }
        RELATIVE_JOINING => FetchParams::RelativeJoining {
            joining_request_id: get_vi64(buf)?,
            joining_start: get_vi64(buf)?,
        },
        ABSOLUTE_JOINING => FetchParams::AbsoluteJoining {
            joining_request_id: get_vi64(buf)?,
            joining_start: get_vi64(buf)?,
        },
        _ => return Err(DecodeError::protocol_violation("unknown fetch type")),
    };
    let mut fetch = Fetch {
        request_id,
        subscriber_priority: DEFAULT_SUBSCRIBER_PRIORITY,
        // §10.2.8: an absent GROUP_ORDER in FETCH is Ascending.
        group_order: GroupOrder::Ascending,
        fetch_params,
        authorization_tokens: vec![],
        fill_timeout: None,
    };
    for parameter in get_parameters(buf, FETCH_PARAMETERS)? {
        match parameter {
            Parameter::SubscriberPriority(priority) => fetch.subscriber_priority = priority,
            Parameter::GroupOrder(order) => fetch.group_order = order,
            Parameter::AuthorizationToken(token) => fetch.authorization_tokens.push(token),
            Parameter::FillTimeout(milliseconds) => fetch.fill_timeout = Some(milliseconds),
            _ => {}
        }
    }
    Ok(fetch)
}

pub(crate) fn encode_fetch(fetch: &Fetch) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, fetch.request_id);
    match &fetch.fetch_params {
        FetchParams::Standalone {
            track_namespace,
            track_name,
            start_location,
            end_location,
        } => {
            put_vi64(&mut payload, STANDALONE);
            put_full_track_name(&mut payload, track_namespace, track_name);
            put_location(&mut payload, start_location);
            put_location(&mut payload, end_location);
        }
        FetchParams::RelativeJoining {
            joining_request_id,
            joining_start,
        } => {
            put_vi64(&mut payload, RELATIVE_JOINING);
            put_vi64(&mut payload, *joining_request_id);
            put_vi64(&mut payload, *joining_start);
        }
        FetchParams::AbsoluteJoining {
            joining_request_id,
            joining_start,
        } => {
            put_vi64(&mut payload, ABSOLUTE_JOINING);
            put_vi64(&mut payload, *joining_request_id);
            put_vi64(&mut payload, *joining_start);
        }
    }
    let mut parameters = vec![Parameter::SubscriberPriority(fetch.subscriber_priority)];
    if fetch.group_order != GroupOrder::Publisher {
        parameters.push(Parameter::GroupOrder(fetch.group_order));
    }
    parameters.extend(fetch.fill_timeout.map(Parameter::FillTimeout));
    parameters.extend(token_parameters(&fetch.authorization_tokens));
    put_parameters(&mut payload, &parameters)?;
    frame(FETCH, &payload)
}

fn encode_namespace_request(
    message_type: u64,
    request_id: u64,
    namespace: &[String],
    tokens: &[AuthorizationToken],
) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, request_id);
    put_track_namespace(&mut payload, namespace);
    put_parameters(&mut payload, &token_parameters(tokens))?;
    frame(message_type, &payload)
}

pub(crate) fn encode_publish_namespace(message: &PublishNamespace) -> anyhow::Result<BytesMut> {
    encode_namespace_request(
        PUBLISH_NAMESPACE,
        message.request_id,
        &message.track_namespace,
        &message.authorization_token,
    )
}

pub(crate) fn encode_subscribe_namespace(message: &SubscribeNamespace) -> anyhow::Result<BytesMut> {
    encode_namespace_request(
        SUBSCRIBE_NAMESPACE,
        message.request_id,
        &message.track_namespace_prefix,
        &message.authorization_token,
    )
}

/// FORWARD is omitted when it is 1, its default (§10.2.12).
pub(crate) fn encode_subscribe_tracks(
    message: &SubscribeTracksMessage,
) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, message.request_id);
    put_track_namespace(&mut payload, &message.track_namespace_prefix);
    let mut parameters = token_parameters(&message.authorization_tokens);
    if !message.forward {
        parameters.push(Parameter::Forward(false));
    }
    put_parameters(&mut payload, &parameters)?;
    frame(SUBSCRIBE_TRACKS, &payload)
}

/// NAMESPACE or NAMESPACE_DONE, which carry the namespace without the
/// subscription's prefix (§10.16, §10.17).
pub(crate) fn encode_namespace(message_type: u64, suffix: &[String]) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_track_namespace(&mut payload, suffix);
    frame(message_type, &payload)
}

pub(crate) fn encode_subscribe_ok(subscribe_ok: &SubscribeOk) -> anyhow::Result<BytesMut> {
    let mut parameters = Vec::new();
    if subscribe_ok.expires != 0 {
        parameters.push(Parameter::Expires(subscribe_ok.expires));
    }
    parameters.extend(largest_object(&subscribe_ok.content_exists));
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, subscribe_ok.track_alias);
    put_parameters(&mut payload, &parameters)?;
    put_key_value_pairs(
        &mut payload,
        &track_properties(
            subscribe_ok.delivery_timeout,
            subscribe_ok.max_duration,
            subscribe_ok.group_order.delivered(),
        ),
    )?;
    frame(SUBSCRIBE_OK, &payload)
}

impl SubscribeOkMessage {
    pub(crate) fn into_subscribe_ok(self, request_id: u64) -> SubscribeOk {
        let expires = self
            .parameters
            .iter()
            .find_map(|parameter| match parameter {
                Parameter::Expires(expires) => Some(*expires),
                _ => None,
            })
            .unwrap_or(0);
        SubscribeOk {
            request_id,
            track_alias: self.track_alias,
            expires,
            group_order: property_group_order(&self.track_properties),
            content_exists: content_exists(&self.parameters),
            delivery_timeout: varint_property_value(
                &self.track_properties,
                OBJECT_DELIVERY_TIMEOUT_PROPERTY,
            ),
            max_duration: varint_property_value(
                &self.track_properties,
                MAX_CACHE_DURATION_PROPERTY,
            ),
        }
    }
}

fn encode_request_ok(
    parameters: &[Parameter],
    properties: &[KeyValuePair],
) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_parameters(&mut payload, parameters)?;
    put_key_value_pairs(&mut payload, properties)?;
    frame(REQUEST_OK, &payload)
}

/// PUBLISH_NAMESPACE_OK and SUBSCRIBE_NAMESPACE_OK carry nothing the model
/// sets.
pub(crate) fn encode_empty_request_ok() -> anyhow::Result<BytesMut> {
    encode_request_ok(&[], &[])
}

/// REQUEST_UPDATE_OK carries the publisher's Largest Object (§10.2.11).
pub(crate) fn encode_request_update_ok(largest: Option<Location>) -> anyhow::Result<BytesMut> {
    encode_request_ok(largest.map(Parameter::LargestObject).as_slice(), &[])
}

pub(crate) fn encode_publish_ok(publish_ok: &PublishOk) -> anyhow::Result<BytesMut> {
    let mut parameters = vec![
        Parameter::Forward(publish_ok.forward),
        Parameter::SubscriberPriority(publish_ok.subscriber_priority),
        Parameter::SubscriptionFilter(publish_ok.filter_type),
    ];
    if publish_ok.group_order != GroupOrder::Publisher {
        parameters.push(Parameter::GroupOrder(publish_ok.group_order));
    }
    if let Some(timeout) = publish_ok.delivery_timeout {
        parameters.push(Parameter::ObjectDeliveryTimeout(timeout));
    }
    encode_request_ok(&parameters, &[])
}

/// TRACK_STATUS_OK is a REQUEST_OK with the Track Properties a SUBSCRIBE_OK
/// would carry (§10.14); of its parameters only LARGEST_OBJECT is allowed
/// here, EXPIRES is not (§10.2.10).
pub(crate) fn encode_track_status_ok(track_status_ok: &SubscribeOk) -> anyhow::Result<BytesMut> {
    encode_request_ok(
        largest_object(&track_status_ok.content_exists).as_slice(),
        &track_properties(
            track_status_ok.delivery_timeout,
            track_status_ok.max_duration,
            track_status_ok.group_order.delivered(),
        ),
    )
}

impl RequestOkMessage {
    pub(crate) fn into_publish_ok(self, request_id: u64) -> PublishOk {
        let mut publish_ok = PublishOk {
            request_id,
            forward: true,
            subscriber_priority: DEFAULT_SUBSCRIBER_PRIORITY,
            group_order: GroupOrder::Publisher,
            filter_type: UNFILTERED,
            delivery_timeout: None,
        };
        for parameter in self.parameters {
            match parameter {
                Parameter::Forward(forward) => publish_ok.forward = forward,
                Parameter::SubscriberPriority(priority) => {
                    publish_ok.subscriber_priority = priority
                }
                Parameter::GroupOrder(order) => publish_ok.group_order = order,
                Parameter::SubscriptionFilter(filter) => publish_ok.filter_type = filter,
                Parameter::ObjectDeliveryTimeout(timeout) => {
                    publish_ok.delivery_timeout = Some(timeout)
                }
                _ => {}
            }
        }
        publish_ok
    }

    /// Track Alias is not used for TRACK_STATUS (§10.14).
    pub(crate) fn into_track_status_ok(self, request_id: u64) -> SubscribeOk {
        SubscribeOkMessage {
            track_alias: 0,
            parameters: self.parameters,
            track_properties: self.track_properties,
        }
        .into_subscribe_ok(request_id)
    }
}

pub(crate) fn encode_fetch_ok(fetch_ok: &FetchOk) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    payload.put_u8(u8::from(fetch_ok.end_of_track));
    put_location(&mut payload, &fetch_ok.end_location);
    put_parameters(&mut payload, &[])?;
    put_key_value_pairs(
        &mut payload,
        &track_properties(None, fetch_ok.max_cache_duration, GroupOrder::Publisher),
    )?;
    frame(FETCH_OK, &payload)
}

impl FetchOkMessage {
    /// FETCH_OK states no group order; the response follows the FETCH's.
    pub(crate) fn into_fetch_ok(self, request_id: u64, group_order: GroupOrder) -> FetchOk {
        FetchOk {
            request_id,
            group_order: group_order.delivered(),
            end_of_track: self.end_of_track,
            end_location: self.end_location,
            max_cache_duration: varint_property_value(
                &self.track_properties,
                MAX_CACHE_DURATION_PROPERTY,
            ),
        }
    }
}

fn decode_request_error(buf: &mut Bytes) -> Result<RequestErrorMessage, DecodeError> {
    let code = request_error_code_from_wire(get_vi64(buf)?);
    // Retry Interval: nothing retries a failed request.
    get_vi64(buf)?;
    let reason = get_reason_phrase(buf)?;
    // Read so that a Redirect structure counts toward the length check.
    if code == RequestErrorCode::Redirect {
        get_uri(buf)?;
        get_full_track_name(buf)?;
    }
    Ok(RequestErrorMessage { code, reason })
}

/// Sent with Retry Interval 0: the request SHOULD NOT be retried (§10.6).
pub(crate) fn encode_request_error(
    code: RequestErrorCode,
    reason: &str,
) -> anyhow::Result<BytesMut> {
    // REDIRECT needs a Redirect structure the model cannot supply.
    let code = match code {
        RequestErrorCode::Redirect => RequestErrorCode::InternalError,
        code => code,
    };
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, request_error_code_to_wire(code));
    put_vi64(&mut payload, 0);
    put_reason_phrase(&mut payload, reason);
    frame(REQUEST_ERROR, &payload)
}

pub(crate) fn encode_publish_done(
    code: PublishDoneCode,
    stream_count: u64,
    reason: &str,
) -> anyhow::Result<BytesMut> {
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, publish_done_code_to_wire(code));
    put_vi64(&mut payload, stream_count);
    put_reason_phrase(&mut payload, reason);
    frame(PUBLISH_DONE, &payload)
}

/// The model's update states the whole range: an `end_group` of 0 is open
/// ended, otherwise it is the last group plus 1.
pub(crate) fn encode_request_update(
    request_id: u64,
    update: &SubscribeUpdate,
) -> anyhow::Result<BytesMut> {
    let filter = match update.end_group {
        0 => FilterType::AbsoluteStart {
            location: update.start_location,
        },
        end_group => FilterType::AbsoluteRange {
            location: update.start_location,
            end_group: end_group - 1,
        },
    };
    let mut parameters = vec![
        Parameter::SubscriberPriority(update.subscriber_priority),
        Parameter::Forward(update.forward),
        Parameter::SubscriptionFilter(filter),
    ];
    if let Some(timeout) = update.delivery_timeout {
        parameters.push(Parameter::ObjectDeliveryTimeout(timeout));
    }
    parameters.extend(token_parameters(&update.authorization_tokens));
    let mut payload = BytesMut::new();
    put_vi64(&mut payload, request_id);
    put_parameters(&mut payload, &parameters)?;
    frame(REQUEST_UPDATE, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::moqt::control_plane::constants::TerminationErrorCode;

    fn location(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

    fn decode(framed: BytesMut) -> Message {
        let mut buf = framed;
        let (message_type, payload) = take_message(&mut buf).unwrap();
        assert!(buf.is_empty());
        decode_message(message_type, payload).unwrap()
    }

    fn subscribe() -> Subscribe {
        Subscribe {
            request_id: 4,
            track_namespace: vec!["live".to_string(), "room".to_string()],
            track_name: "video".to_string(),
            subscriber_priority: 7,
            group_order: GroupOrder::Descending,
            forward: false,
            filter_type: FilterType::AbsoluteRange {
                location: location(3, 1),
                end_group: 9,
            },
            authorization_tokens: vec![AuthorizationToken::use_value_utf8("jwt")],
            delivery_timeout: Some(1500),
        }
    }

    #[test]
    fn setup_round_trips() {
        // Arrange
        let setup = SetupMessage {
            path: Some("/moq".to_string()),
            authority: Some("relay.example:443".to_string()),
            authorization_tokens: vec![AuthorizationToken::use_value_utf8("jwt")],
            max_auth_token_cache_size: 0,
            implementation: Some("moq-wasm".to_string()),
        };

        // Act
        let decoded = decode(encode_setup(&setup).unwrap());

        // Assert
        assert_eq!(decoded, Message::Setup(setup));
    }

    #[test]
    fn setup_ignores_unknown_options() {
        // Arrange: PATH between an unknown varint option and an unknown bytes option
        let mut payload = BytesMut::new();
        let options = [
            varint_property(0x00, 7),
            KeyValuePair {
                key: PATH_OPTION,
                value: VariantType::Odd(Bytes::from_static(b"/moq")),
            },
            KeyValuePair {
                key: 0x9D,
                value: VariantType::Odd(Bytes::from_static(b"grease")),
            },
        ];
        put_key_value_pairs(&mut payload, &options).unwrap();

        // Act
        let decoded = decode(frame(SETUP, &payload).unwrap());

        // Assert: §10.3
        assert_eq!(
            decoded,
            Message::Setup(SetupMessage {
                path: Some("/moq".to_string()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn requests_round_trip_through_the_shared_model() {
        // Arrange
        let publish = Publish {
            request_id: 2,
            track_namespace_tuple: vec!["live".to_string()],
            track_name: "audio".to_string(),
            track_alias: 11,
            group_order: GroupOrder::Descending,
            content_exists: ContentExists::True {
                location: location(5, 6),
            },
            forward: true,
            authorization_tokens: vec![],
            delivery_timeout: Some(200),
            max_duration: Some(60_000),
        };
        let fetch = Fetch {
            request_id: 6,
            subscriber_priority: 9,
            group_order: GroupOrder::Descending,
            fetch_params: FetchParams::Standalone {
                track_namespace: vec!["live".to_string()],
                track_name: "video".to_string(),
                start_location: location(1, 0),
                end_location: location(4, 0),
            },
            authorization_tokens: vec![],
            fill_timeout: None,
        };
        let joining = Fetch {
            fetch_params: FetchParams::RelativeJoining {
                joining_request_id: 4,
                joining_start: 2,
            },
            ..fetch.clone()
        };
        let publish_namespace = PublishNamespace {
            request_id: 8,
            track_namespace: vec!["live".to_string()],
            authorization_token: vec![AuthorizationToken::use_value_utf8("jwt")],
        };
        let subscribe_namespace = SubscribeNamespace {
            request_id: 10,
            track_namespace_prefix: vec![],
            authorization_token: vec![],
        };

        // Act / Assert
        assert_eq!(
            decode(encode_subscribe(&subscribe()).unwrap()),
            Message::Subscribe(subscribe())
        );
        let Message::Publish {
            publish: decoded_publish,
            ..
        } = decode(encode_publish(&publish).unwrap())
        else {
            panic!("expected PUBLISH");
        };
        assert_eq!(decoded_publish, publish);
        assert_eq!(decode(encode_fetch(&fetch).unwrap()), Message::Fetch(fetch));
        assert_eq!(
            decode(encode_fetch(&joining).unwrap()),
            Message::Fetch(joining)
        );
        assert_eq!(
            decode(encode_publish_namespace(&publish_namespace).unwrap()),
            Message::PublishNamespace(publish_namespace)
        );
        assert_eq!(
            decode(encode_subscribe_namespace(&subscribe_namespace).unwrap()),
            Message::SubscribeNamespace(subscribe_namespace)
        );
    }

    #[test]
    fn subscribe_tracks_round_trips_with_its_forward_state() {
        for forward in [false, true] {
            // Arrange
            let subscribe_tracks = SubscribeTracksMessage {
                request_id: 12,
                track_namespace_prefix: vec!["live".to_string()],
                authorization_tokens: vec![AuthorizationToken::use_value_utf8("jwt")],
                forward,
            };

            // Act
            let decoded = decode(encode_subscribe_tracks(&subscribe_tracks).unwrap());

            // Assert
            assert_eq!(decoded, Message::SubscribeTracks(subscribe_tracks));
        }
    }

    #[test]
    fn a_fetch_carries_its_fill_timeout() {
        for fill_timeout in [Some(0), Some(1500), None] {
            // Arrange
            let fetch = Fetch {
                request_id: 6,
                subscriber_priority: 9,
                group_order: GroupOrder::Ascending,
                fetch_params: FetchParams::Standalone {
                    track_namespace: vec!["live".to_string()],
                    track_name: "video".to_string(),
                    start_location: location(1, 0),
                    end_location: location(4, 0),
                },
                authorization_tokens: vec![],
                fill_timeout,
            };

            // Act
            let decoded = decode(encode_fetch(&fetch).unwrap());

            // Assert: §10.2.5
            assert_eq!(decoded, Message::Fetch(fetch), "{fill_timeout:?}");
        }
    }

    #[test]
    fn track_status_carries_only_its_tokens() {
        // Act
        let decoded = decode(encode_track_status(&subscribe()).unwrap());

        // Assert: defaults stand in for the parameters TRACK_STATUS omits
        let Message::TrackStatus(track_status) = decoded else {
            panic!("expected TRACK_STATUS");
        };
        assert_eq!(
            track_status.authorization_tokens,
            subscribe().authorization_tokens
        );
        assert_eq!(track_status.subscriber_priority, 128);
        assert!(track_status.forward);
    }

    #[test]
    fn track_status_with_a_subscribe_delivery_parameter_is_a_protocol_violation() {
        // Arrange: a SUBSCRIBE payload, which carries SUBSCRIBER_PRIORITY
        let mut framed = encode_subscribe(&subscribe()).unwrap();
        let (_, payload) = take_message(&mut framed).unwrap();

        // Act
        let result = decode_message(TRACK_STATUS, payload);

        // Assert: §10.2.1, §10.2.7
        assert!(
            matches!(
                result,
                Err(DecodeError::Violation {
                    code: TerminationErrorCode::ProtocolViolation,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn responses_map_back_to_the_shared_model_with_the_request_id() {
        // Arrange
        let subscribe_ok = SubscribeOk {
            request_id: 4,
            track_alias: 3,
            expires: 30_000,
            group_order: GroupOrder::Descending,
            content_exists: ContentExists::True {
                location: location(8, 2),
            },
            delivery_timeout: Some(100),
            max_duration: Some(5_000),
        };
        let publish_ok = PublishOk {
            request_id: 2,
            forward: false,
            subscriber_priority: 3,
            group_order: GroupOrder::Ascending,
            filter_type: FilterType::LargestObject,
            delivery_timeout: Some(50),
        };
        let fetch_ok = FetchOk {
            request_id: 6,
            group_order: GroupOrder::Descending,
            end_of_track: true,
            end_location: location(4, 1),
            max_cache_duration: Some(10),
        };

        // Act
        let Message::SubscribeOk(decoded_subscribe_ok) =
            decode(encode_subscribe_ok(&subscribe_ok).unwrap())
        else {
            panic!("expected SUBSCRIBE_OK");
        };
        let Message::RequestOk(decoded_publish_ok) =
            decode(encode_publish_ok(&publish_ok).unwrap())
        else {
            panic!("expected REQUEST_OK");
        };
        let Message::RequestOk(decoded_track_status_ok) =
            decode(encode_track_status_ok(&subscribe_ok).unwrap())
        else {
            panic!("expected REQUEST_OK");
        };
        let Message::FetchOk(decoded_fetch_ok) = decode(encode_fetch_ok(&fetch_ok).unwrap()) else {
            panic!("expected FETCH_OK");
        };

        // Assert
        assert_eq!(decoded_subscribe_ok.into_subscribe_ok(4), subscribe_ok);
        assert_eq!(decoded_publish_ok.into_publish_ok(2), publish_ok);
        assert_eq!(
            decoded_track_status_ok.into_track_status_ok(4),
            SubscribeOk {
                track_alias: 0,
                expires: 0,
                ..subscribe_ok
            }
        );
        assert_eq!(
            decoded_fetch_ok.into_fetch_ok(6, GroupOrder::Descending),
            fetch_ok
        );
    }

    #[test]
    fn request_error_and_publish_done_carry_draft18_codes() {
        // Act
        let error =
            decode(encode_request_error(RequestErrorCode::TrackDoesNotExist, "gone").unwrap());
        let done = decode(encode_publish_done(PublishDoneCode::UpdateFailed, 0, "bye").unwrap());

        // Assert
        let Message::RequestError(error) = error else {
            panic!("expected REQUEST_ERROR");
        };
        assert_eq!(error.code, RequestErrorCode::TrackDoesNotExist);
        assert_eq!(error.reason, "gone");
        let Message::PublishDone(done) = done else {
            panic!("expected PUBLISH_DONE");
        };
        assert_eq!(done.code, PublishDoneCode::UpdateFailed);
    }

    #[test]
    fn a_request_update_states_the_whole_range() {
        // Arrange
        let update = SubscribeUpdate {
            request_id: 12,
            subscription_request_id: 4,
            start_location: location(5, 0),
            end_group: 8,
            subscriber_priority: 1,
            forward: true,
            authorization_tokens: vec![],
            delivery_timeout: None,
        };

        // Act
        let decoded = decode(encode_request_update(12, &update).unwrap());

        // Assert: end_group 8 (last + 1) is the inclusive end group 7
        let Message::RequestUpdate(decoded) = decoded else {
            panic!("expected REQUEST_UPDATE");
        };
        assert_eq!(decoded.request_id, 12);
        assert!(decoded.parameters.contains(&Parameter::SubscriptionFilter(
            FilterType::AbsoluteRange {
                location: location(5, 0),
                end_group: 7,
            }
        )));
    }

    #[test]
    fn go_away_reads_its_request_id_only_when_present() {
        for request_id in [None, Some(3)] {
            // Arrange
            let go_away = GoAwayMessage {
                new_session_uri: String::new(),
                request_id,
            };

            // Act / Assert
            assert_eq!(
                decode(encode_go_away(&go_away).unwrap()),
                Message::GoAway(go_away)
            );
        }
    }

    #[test]
    fn the_legacy_publish_ok_type_reads_as_request_ok() {
        // Act
        let decoded = decode_message(PUBLISH_OK, Bytes::from_static(&[0])).unwrap();

        // Assert
        assert_eq!(
            decoded,
            Message::RequestOk(RequestOkMessage {
                parameters: vec![],
                track_properties: vec![],
            })
        );
    }

    #[test]
    fn a_partial_frame_needs_more_data_and_leaves_the_buffer() {
        // Arrange
        let framed = frame(REQUEST_UPDATE, &[1, 2, 3]).unwrap();
        let mut partial = BytesMut::from(&framed[..framed.len() - 1]);

        // Act
        let result = take_message(&mut partial);

        // Assert
        assert_eq!(result, Err(DecodeError::NeedMoreData));
        assert_eq!(partial.len(), framed.len() - 1);
    }

    #[test]
    fn unknown_types_length_mismatches_and_misplaced_parameters_are_protocol_violations() {
        // Arrange: FETCH_OK with a parameter (FETCH_OK allows none)
        let mut fetch_ok_with_parameter = BytesMut::new();
        fetch_ok_with_parameter.extend_from_slice(&[0, 1, 0]);
        put_vi64(&mut fetch_ok_with_parameter, 1);
        put_vi64(&mut fetch_ok_with_parameter, 0x08);
        put_vi64(&mut fetch_ok_with_parameter, 1);
        let cases = [
            (0x40, Bytes::new()),
            (REQUEST_UPDATE, Bytes::from_static(&[1])),
            (FETCH_OK, fetch_ok_with_parameter.freeze()),
        ];
        for (message_type, payload) in cases {
            // Act
            let result = decode_message(message_type, payload);

            // Assert
            assert!(
                matches!(
                    result,
                    Err(DecodeError::Violation {
                        code: TerminationErrorCode::ProtocolViolation,
                        ..
                    })
                ),
                "{message_type:#x}: {result:?}"
            );
        }
    }

    #[test]
    fn a_mandatory_track_property_is_recognised() {
        // Arrange
        let properties = [KeyValuePair {
            key: 0x4000,
            value: VariantType::Even(1),
        }];

        // Act / Assert
        assert!(has_mandatory_track_property(&properties));
        assert!(!has_mandatory_track_property(&[]));
    }

    #[test]
    fn a_default_publisher_priority_above_255_is_a_protocol_violation() {
        // Arrange: DEFAULT PUBLISHER PRIORITY (0x0E) of 256, in the mutable
        // list, inside Immutable Properties, and after a valid one
        let cases = [
            vec![varint_property(0x0E, 256)],
            vec![varint_property(0x0E, 9), varint_property(0x0E, 256)],
            vec![
                immutable(&[varint_property(0x0E, 256)]),
                varint_property(0x0E, 9),
            ],
        ];
        for properties in cases {
            let mut payload = BytesMut::new();
            put_vi64(&mut payload, 1);
            put_parameters(&mut payload, &[]).unwrap();
            put_key_value_pairs(&mut payload, &properties).unwrap();

            // Act
            let result = decode_message(SUBSCRIBE_OK, payload.freeze());

            // Assert: §12.4
            assert!(
                matches!(
                    result,
                    Err(DecodeError::Violation {
                        code: TerminationErrorCode::ProtocolViolation,
                        ..
                    })
                ),
                "{properties:?}: {result:?}"
            );
        }
    }

    #[test]
    fn a_default_publisher_group_order_out_of_range_is_a_protocol_violation() {
        // Arrange: DEFAULT PUBLISHER GROUP ORDER (0x22) of 3 or 0, in the
        // mutable list, inside Immutable Properties, and before a valid one
        let cases = [
            vec![varint_property(0x22, 3)],
            vec![varint_property(0x22, 0)],
            vec![immutable(&[varint_property(0x22, 3)])],
            vec![
                immutable(&[varint_property(0x22, 3)]),
                varint_property(0x22, 1),
            ],
        ];
        for properties in cases {
            let mut payload = BytesMut::new();
            put_vi64(&mut payload, 1);
            put_parameters(&mut payload, &[]).unwrap();
            put_key_value_pairs(&mut payload, &properties).unwrap();

            // Act
            let result = decode_message(SUBSCRIBE_OK, payload.freeze());

            // Assert: §12.5
            assert!(
                matches!(
                    result,
                    Err(DecodeError::Violation {
                        code: TerminationErrorCode::ProtocolViolation,
                        ..
                    })
                ),
                "{properties:?}: {result:?}"
            );
        }
    }

    /// §12.7 Immutable Properties holding `properties`.
    fn immutable(properties: &[KeyValuePair]) -> KeyValuePair {
        let mut contents = BytesMut::new();
        put_key_value_pairs(&mut contents, properties).unwrap();
        KeyValuePair {
            key: 0x0B,
            value: VariantType::Odd(contents.freeze()),
        }
    }

    #[test]
    fn track_properties_inside_immutable_properties_are_found() {
        // Arrange
        let properties = vec![immutable(&[varint_property(0x04, 50)])];
        let mut subscribe_ok = BytesMut::new();
        put_vi64(&mut subscribe_ok, 1);
        put_parameters(&mut subscribe_ok, &[]).unwrap();
        put_key_value_pairs(&mut subscribe_ok, &properties).unwrap();
        let mut publish = BytesMut::new();
        put_vi64(&mut publish, 1);
        put_full_track_name(&mut publish, &["ns".to_string()], "track");
        put_vi64(&mut publish, 2);
        put_parameters(&mut publish, &[]).unwrap();
        put_key_value_pairs(&mut publish, &properties).unwrap();
        let mut fetch_ok = BytesMut::new();
        fetch_ok.put_u8(0);
        put_location(&mut fetch_ok, &location(1, 0));
        put_parameters(&mut fetch_ok, &[]).unwrap();
        put_key_value_pairs(&mut fetch_ok, &properties).unwrap();

        // Act
        let subscribe_ok = decode(frame(SUBSCRIBE_OK, &subscribe_ok).unwrap());
        let publish = decode(frame(PUBLISH, &publish).unwrap());
        let fetch_ok = decode(frame(FETCH_OK, &fetch_ok).unwrap());

        // Assert: §12.7
        let Message::SubscribeOk(subscribe_ok) = subscribe_ok else {
            panic!("expected SUBSCRIBE_OK");
        };
        assert_eq!(subscribe_ok.into_subscribe_ok(1).max_duration, Some(50));
        let Message::Publish { publish, .. } = publish else {
            panic!("expected PUBLISH");
        };
        assert_eq!(publish.max_duration, Some(50));
        let Message::FetchOk(fetch_ok) = fetch_ok else {
            panic!("expected FETCH_OK");
        };
        assert_eq!(
            fetch_ok
                .into_fetch_ok(1, GroupOrder::Ascending)
                .max_cache_duration,
            Some(50)
        );
        assert_eq!(
            default_publisher_priority(&[immutable(&[varint_property(0x0E, 9)])]),
            9
        );
        assert!(has_mandatory_track_property(&[immutable(&[
            varint_property(0x4000, 1)
        ])]));
    }
}

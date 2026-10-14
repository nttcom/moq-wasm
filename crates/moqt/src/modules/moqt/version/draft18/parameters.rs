//! Message Parameters (§10.2), the Token structure of the AUTHORIZATION
//! TOKEN parameter (§10.2.2) and the Subscription Filter (§5.1.2).

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::codec::{
    DecodeError, get_location, get_track_namespace, get_vi64, put_location, put_track_namespace,
    put_vi64,
};
use crate::modules::moqt::control_plane::{
    constants::TerminationErrorCode,
    control_messages::messages::parameters::{
        authorization_token::AuthorizationToken, filter_type::FilterType, group_order::GroupOrder,
        location::Location,
    },
};

const OBJECT_DELIVERY_TIMEOUT: u64 = 0x02;
const AUTHORIZATION_TOKEN: u64 = 0x03;
const RENDEZVOUS_TIMEOUT: u64 = 0x04;
const SUBGROUP_DELIVERY_TIMEOUT: u64 = 0x06;
const EXPIRES: u64 = 0x08;
const LARGEST_OBJECT: u64 = 0x09;
const FILL_TIMEOUT: u64 = 0x0A;
const FORWARD: u64 = 0x10;
const SUBSCRIBER_PRIORITY: u64 = 0x20;
const SUBSCRIPTION_FILTER: u64 = 0x21;
const GROUP_ORDER: u64 = 0x22;
const NEW_GROUP_REQUEST: u64 = 0x32;
const TRACK_NAMESPACE_PREFIX: u64 = 0x34;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Parameter {
    ObjectDeliveryTimeout(u64),
    AuthorizationToken(AuthorizationToken),
    RendezvousTimeout(u64),
    SubgroupDeliveryTimeout(u64),
    Expires(u64),
    LargestObject(Location),
    FillTimeout(u64),
    Forward(bool),
    SubscriberPriority(u8),
    SubscriptionFilter(FilterType),
    GroupOrder(GroupOrder),
    NewGroupRequest(u64),
    TrackNamespacePrefix(Vec<String>),
}

impl Parameter {
    pub(crate) fn kind(&self) -> u64 {
        match self {
            Self::ObjectDeliveryTimeout(_) => OBJECT_DELIVERY_TIMEOUT,
            Self::AuthorizationToken(_) => AUTHORIZATION_TOKEN,
            Self::RendezvousTimeout(_) => RENDEZVOUS_TIMEOUT,
            Self::SubgroupDeliveryTimeout(_) => SUBGROUP_DELIVERY_TIMEOUT,
            Self::Expires(_) => EXPIRES,
            Self::LargestObject(_) => LARGEST_OBJECT,
            Self::FillTimeout(_) => FILL_TIMEOUT,
            Self::Forward(_) => FORWARD,
            Self::SubscriberPriority(_) => SUBSCRIBER_PRIORITY,
            Self::SubscriptionFilter(_) => SUBSCRIPTION_FILTER,
            Self::GroupOrder(_) => GROUP_ORDER,
            Self::NewGroupRequest(_) => NEW_GROUP_REQUEST,
            Self::TrackNamespacePrefix(_) => TRACK_NAMESPACE_PREFIX,
        }
    }

    fn put_value(&self, buf: &mut BytesMut) -> anyhow::Result<()> {
        match self {
            Self::ObjectDeliveryTimeout(value)
            | Self::RendezvousTimeout(value)
            | Self::SubgroupDeliveryTimeout(value)
            | Self::Expires(value)
            | Self::FillTimeout(value)
            | Self::NewGroupRequest(value) => put_vi64(buf, *value),
            Self::AuthorizationToken(token) => {
                let token = encode_token(token)?;
                put_vi64(buf, token.len() as u64);
                buf.put_slice(&token);
            }
            Self::LargestObject(location) => put_location(buf, location),
            Self::Forward(forward) => buf.put_u8(u8::from(*forward)),
            Self::SubscriberPriority(priority) => buf.put_u8(*priority),
            Self::SubscriptionFilter(filter) => {
                let filter = encode_filter(filter)?;
                put_vi64(buf, filter.len() as u64);
                buf.put_slice(&filter);
            }
            Self::GroupOrder(order) => buf.put_u8(match order {
                GroupOrder::Ascending => 0x1,
                GroupOrder::Descending => 0x2,
                GroupOrder::Publisher => {
                    anyhow::bail!("draft-18 omits GROUP_ORDER for the publisher's order")
                }
            }),
            Self::TrackNamespacePrefix(prefix) => put_track_namespace(buf, prefix),
        }
        Ok(())
    }
}

fn protocol_violation(reason: &'static str) -> DecodeError {
    DecodeError::Violation {
        code: TerminationErrorCode::ProtocolViolation,
        reason,
    }
}

fn get_u8(buf: &mut impl Buf) -> Result<u8, DecodeError> {
    buf.try_get_u8().map_err(|_| DecodeError::NeedMoreData)
}

fn get_length_prefixed(buf: &mut impl Buf) -> Result<Bytes, DecodeError> {
    let len = get_vi64(buf)? as usize;
    if buf.remaining() < len {
        return Err(DecodeError::NeedMoreData);
    }
    Ok(buf.copy_to_bytes(len))
}

fn get_parameter(kind: u64, buf: &mut impl Buf) -> Result<Parameter, DecodeError> {
    Ok(match kind {
        OBJECT_DELIVERY_TIMEOUT => Parameter::ObjectDeliveryTimeout(get_vi64(buf)?),
        AUTHORIZATION_TOKEN => {
            Parameter::AuthorizationToken(decode_token(get_length_prefixed(buf)?)?)
        }
        RENDEZVOUS_TIMEOUT => Parameter::RendezvousTimeout(get_vi64(buf)?),
        SUBGROUP_DELIVERY_TIMEOUT => Parameter::SubgroupDeliveryTimeout(get_vi64(buf)?),
        EXPIRES => Parameter::Expires(get_vi64(buf)?),
        LARGEST_OBJECT => Parameter::LargestObject(get_location(buf)?),
        FILL_TIMEOUT => Parameter::FillTimeout(get_vi64(buf)?),
        FORWARD => Parameter::Forward(match get_u8(buf)? {
            0 => false,
            1 => true,
            _ => return Err(protocol_violation("FORWARD other than 0 or 1")),
        }),
        SUBSCRIBER_PRIORITY => Parameter::SubscriberPriority(get_u8(buf)?),
        SUBSCRIPTION_FILTER => {
            Parameter::SubscriptionFilter(decode_filter(&mut get_length_prefixed(buf)?)?)
        }
        GROUP_ORDER => Parameter::GroupOrder(match get_u8(buf)? {
            0x1 => GroupOrder::Ascending,
            0x2 => GroupOrder::Descending,
            _ => return Err(protocol_violation("GROUP_ORDER other than 1 or 2")),
        }),
        NEW_GROUP_REQUEST => Parameter::NewGroupRequest(get_vi64(buf)?),
        TRACK_NAMESPACE_PREFIX => Parameter::TrackNamespacePrefix(get_track_namespace(buf)?),
        _ => return Err(protocol_violation("unknown message parameter")),
    })
}

/// Reads `Number of Parameters` and the parameters. An unknown, repeated
/// (other than AUTHORIZATION TOKEN) or descending type is a protocol
/// violation.
pub(crate) fn get_parameters(buf: &mut impl Buf) -> Result<Vec<Parameter>, DecodeError> {
    let count = get_vi64(buf)?;
    let mut parameters = Vec::new();
    let mut kind = 0u64;
    for index in 0..count {
        let delta = get_vi64(buf)?;
        if index > 0 && delta == 0 && kind != AUTHORIZATION_TOKEN {
            return Err(protocol_violation("repeated message parameter"));
        }
        kind = kind
            .checked_add(delta)
            .ok_or(protocol_violation("message parameter type overflows"))?;
        parameters.push(get_parameter(kind, buf)?);
    }
    Ok(parameters)
}

/// Writes `Number of Parameters` and the parameters in ascending type order.
pub(crate) fn put_parameters(buf: &mut BytesMut, parameters: &[Parameter]) -> anyhow::Result<()> {
    let mut sorted: Vec<&Parameter> = parameters.iter().collect();
    sorted.sort_by_key(|parameter| parameter.kind());
    put_vi64(buf, sorted.len() as u64);
    let mut prev = 0u64;
    for parameter in sorted {
        put_vi64(buf, parameter.kind() - prev);
        parameter.put_value(buf)?;
        prev = parameter.kind();
    }
    Ok(())
}

/// Fails with PROTOCOL_VIOLATION when a parameter is not one `allowed`
/// lists for the message (§10.2.1).
pub(crate) fn check_allowed(parameters: &[Parameter], allowed: &[u64]) -> Result<(), DecodeError> {
    if parameters
        .iter()
        .any(|parameter| !allowed.contains(&parameter.kind()))
    {
        return Err(protocol_violation(
            "message parameter not allowed in this message",
        ));
    }
    Ok(())
}

const DELETE: u64 = 0x0;
const REGISTER: u64 = 0x1;
const USE_ALIAS: u64 = 0x2;
const USE_VALUE: u64 = 0x3;

/// A Token that cannot be decoded closes the session with
/// KEY_VALUE_FORMATTING_ERROR (§10.2.2).
fn decode_token(mut value: Bytes) -> Result<AuthorizationToken, DecodeError> {
    let formatting = |error: DecodeError| match error {
        DecodeError::NeedMoreData => DecodeError::Violation {
            code: TerminationErrorCode::KeyValueFormattingError,
            reason: "truncated authorization token",
        },
        violation => violation,
    };
    let alias_type = get_vi64(&mut value).map_err(formatting)?;
    Ok(match alias_type {
        DELETE => {
            get_vi64(&mut value).map_err(formatting)?;
            AuthorizationToken::Delete
        }
        REGISTER => AuthorizationToken::Register {
            token_alias: get_vi64(&mut value).map_err(formatting)?,
            token_type: get_vi64(&mut value).map_err(formatting)?,
            token_value: value,
        },
        USE_ALIAS => AuthorizationToken::UseAlias {
            token_alias: get_vi64(&mut value).map_err(formatting)?,
        },
        USE_VALUE => AuthorizationToken::UseValue {
            token_type: get_vi64(&mut value).map_err(formatting)?,
            token_value: value,
        },
        _ => {
            return Err(DecodeError::Violation {
                code: TerminationErrorCode::KeyValueFormattingError,
                reason: "unknown authorization token alias type",
            });
        }
    })
}

/// The model's DELETE has no alias, so it cannot be sent.
fn encode_token(token: &AuthorizationToken) -> anyhow::Result<BytesMut> {
    let mut buf = BytesMut::new();
    match token {
        AuthorizationToken::Delete => anyhow::bail!("cannot send DELETE without its alias"),
        AuthorizationToken::Register {
            token_alias,
            token_type,
            token_value,
        } => {
            put_vi64(&mut buf, REGISTER);
            put_vi64(&mut buf, *token_alias);
            put_vi64(&mut buf, *token_type);
            buf.put_slice(token_value);
        }
        AuthorizationToken::UseAlias { token_alias } => {
            put_vi64(&mut buf, USE_ALIAS);
            put_vi64(&mut buf, *token_alias);
        }
        AuthorizationToken::UseValue {
            token_type,
            token_value,
        } => {
            put_vi64(&mut buf, USE_VALUE);
            put_vi64(&mut buf, *token_type);
            buf.put_slice(token_value);
        }
    }
    Ok(buf)
}

const NEXT_GROUP_START: u64 = 0x1;
const LARGEST_OBJECT_FILTER: u64 = 0x2;
const ABSOLUTE_START: u64 = 0x3;
const ABSOLUTE_RANGE: u64 = 0x4;

/// The model keeps AbsoluteRange's end as a Group ID; draft-18 sends it as
/// End Group Delta from the start group.
fn decode_filter(buf: &mut impl Buf) -> Result<FilterType, DecodeError> {
    Ok(match get_vi64(buf)? {
        NEXT_GROUP_START => FilterType::NextGroupStart,
        LARGEST_OBJECT_FILTER => FilterType::LargestObject,
        ABSOLUTE_START => FilterType::AbsoluteStart {
            location: get_location(buf)?,
        },
        ABSOLUTE_RANGE => {
            let location = get_location(buf)?;
            let end_group = location
                .group_id
                .checked_add(get_vi64(buf)?)
                .ok_or(protocol_violation("filter end group overflows"))?;
            FilterType::AbsoluteRange {
                location,
                end_group,
            }
        }
        _ => return Err(protocol_violation("unknown subscription filter type")),
    })
}

fn encode_filter(filter: &FilterType) -> anyhow::Result<BytesMut> {
    let mut buf = BytesMut::new();
    match filter {
        FilterType::NextGroupStart => put_vi64(&mut buf, NEXT_GROUP_START),
        FilterType::LargestObject => put_vi64(&mut buf, LARGEST_OBJECT_FILTER),
        FilterType::AbsoluteStart { location } => {
            put_vi64(&mut buf, ABSOLUTE_START);
            put_location(&mut buf, location);
        }
        FilterType::AbsoluteRange {
            location,
            end_group,
        } => {
            let delta = end_group
                .checked_sub(location.group_id)
                .ok_or_else(|| anyhow::anyhow!("filter end group precedes its start"))?;
            put_vi64(&mut buf, ABSOLUTE_RANGE);
            put_location(&mut buf, location);
            put_vi64(&mut buf, delta);
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{
        AUTHORIZATION_TOKEN, FORWARD, GROUP_ORDER, Parameter, SUBSCRIPTION_FILTER, check_allowed,
        get_parameters, put_parameters,
    };
    use crate::modules::moqt::{
        control_plane::{
            constants::TerminationErrorCode,
            control_messages::messages::parameters::{
                authorization_token::AuthorizationToken, filter_type::FilterType,
                group_order::GroupOrder, location::Location,
            },
        },
        version::draft18::codec::{DecodeError, put_vi64},
    };

    fn violation(result: Result<Vec<Parameter>, DecodeError>) -> TerminationErrorCode {
        match result {
            Err(DecodeError::Violation { code, .. }) => code,
            other => panic!("expected a violation, got {other:?}"),
        }
    }

    fn round_trip(parameters: &[Parameter]) -> Vec<Parameter> {
        let mut buf = BytesMut::new();
        put_parameters(&mut buf, parameters).unwrap();
        get_parameters(&mut buf.freeze()).unwrap()
    }

    #[test]
    fn every_parameter_round_trips_in_ascending_type_order() {
        // Arrange
        let parameters = vec![
            Parameter::GroupOrder(GroupOrder::Descending),
            Parameter::Forward(false),
            Parameter::SubscriberPriority(7),
            Parameter::SubscriptionFilter(FilterType::AbsoluteRange {
                location: Location {
                    group_id: 10,
                    object_id: 2,
                },
                end_group: 15,
            }),
            Parameter::LargestObject(Location {
                group_id: 3,
                object_id: 4,
            }),
            Parameter::AuthorizationToken(AuthorizationToken::use_value_utf8("jwt")),
            Parameter::AuthorizationToken(AuthorizationToken::use_value_utf8("jwt2")),
            Parameter::ObjectDeliveryTimeout(1000),
            Parameter::SubgroupDeliveryTimeout(2000),
            Parameter::Expires(30_000),
            Parameter::RendezvousTimeout(0),
            Parameter::FillTimeout(5),
            Parameter::NewGroupRequest(0),
            Parameter::TrackNamespacePrefix(vec!["live".to_string()]),
        ];
        let mut expected = parameters.clone();
        expected.sort_by_key(Parameter::kind);

        // Act
        let decoded = round_trip(&parameters);

        // Assert
        assert_eq!(decoded, expected);
    }

    #[test]
    fn an_unknown_parameter_type_is_a_protocol_violation() {
        // Arrange
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, 1);
        put_vi64(&mut buf, 0x05);
        put_vi64(&mut buf, 0);

        // Act / Assert
        assert_eq!(
            violation(get_parameters(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn a_repeated_parameter_other_than_a_token_is_a_protocol_violation() {
        // Arrange: FORWARD twice
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, 2);
        put_vi64(&mut buf, FORWARD);
        buf.extend_from_slice(&[1]);
        put_vi64(&mut buf, 0);
        buf.extend_from_slice(&[1]);

        // Act / Assert
        assert_eq!(
            violation(get_parameters(&mut buf.freeze())),
            TerminationErrorCode::ProtocolViolation
        );
    }

    #[test]
    fn out_of_range_forward_and_group_order_values_are_protocol_violations() {
        for (kind, value) in [(FORWARD, 2u8), (GROUP_ORDER, 0u8)] {
            // Arrange
            let mut buf = BytesMut::new();
            put_vi64(&mut buf, 1);
            put_vi64(&mut buf, kind);
            buf.extend_from_slice(&[value]);

            // Act / Assert
            assert_eq!(
                violation(get_parameters(&mut buf.freeze())),
                TerminationErrorCode::ProtocolViolation,
                "{kind:#x}"
            );
        }
    }

    #[test]
    fn a_truncated_token_is_a_key_value_formatting_error() {
        // Arrange: a USE_VALUE token with no Token Type
        let mut buf = BytesMut::new();
        put_vi64(&mut buf, 1);
        put_vi64(&mut buf, AUTHORIZATION_TOKEN);
        put_vi64(&mut buf, 1);
        put_vi64(&mut buf, 0x3);

        // Act / Assert
        assert_eq!(
            violation(get_parameters(&mut buf.freeze())),
            TerminationErrorCode::KeyValueFormattingError
        );
    }

    #[test]
    fn an_absolute_range_is_sent_as_a_delta_from_its_start_group() {
        // Arrange
        let filter = Parameter::SubscriptionFilter(FilterType::AbsoluteRange {
            location: Location {
                group_id: 10,
                object_id: 0,
            },
            end_group: 12,
        });
        let mut buf = BytesMut::new();

        // Act
        put_parameters(&mut buf, &[filter]).unwrap();

        // Assert: count, type, length, filter type, start {10, 0}, delta 2
        assert_eq!(
            buf.freeze(),
            Bytes::from_static(&[1, SUBSCRIPTION_FILTER as u8, 4, 0x4, 10, 0, 2])
        );
    }

    #[test]
    fn a_parameter_the_message_does_not_allow_is_a_protocol_violation() {
        // Arrange
        let parameters = [Parameter::Expires(1)];

        // Act / Assert
        assert!(matches!(
            check_allowed(&parameters, &[FORWARD]),
            Err(DecodeError::Violation {
                code: TerminationErrorCode::ProtocolViolation,
                ..
            })
        ));
    }
}

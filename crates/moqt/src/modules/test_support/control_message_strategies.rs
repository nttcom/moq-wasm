//! `proptest` strategies that generate draft-14 control messages a peer may
//! send.

use bytes::Bytes;
use proptest::{collection::vec, option, prelude::*};

use crate::modules::moqt::control_plane::control_messages::messages::{
    parameters::{
        authorization_token::AuthorizationToken, filter_type::FilterType, group_order::GroupOrder,
        location::Location, trace_context::TraceContext,
    },
    publish_done::PublishDone,
    subscribe::Subscribe,
    subscribe_namespace::SubscribeNamespace,
    subscribe_update::SubscribeUpdate,
};

/// Values a variable-length integer can carry (draft-14 §1.4.1), drawn evenly
/// across its four encoded lengths, plus the values on either side of each
/// length boundary, which uniform sampling almost never reaches.
pub(crate) fn varint() -> impl Strategy<Value = u64> {
    const LENGTH_EDGES: [u64; 7] = [
        (1 << 6) - 1,
        1 << 6,
        (1 << 14) - 1,
        1 << 14,
        (1 << 30) - 1,
        1 << 30,
        (1 << 62) - 1,
    ];
    prop_oneof![
        0..1u64 << 6,
        1u64 << 6..1 << 14,
        1u64 << 14..1 << 30,
        1u64 << 30..1 << 62,
        proptest::sample::select(&LENGTH_EDGES[..]),
    ]
}

/// Any Unicode text up to 16 characters (64 bytes), so 32 namespace fields
/// and a track name stay within the 4,096-byte Full Track Name (§2.4.1).
fn short_text() -> impl Strategy<Value = String> {
    vec(any::<char>(), 0..=16).prop_map(String::from_iter)
}

/// 1 to 32 fields (§2.4.1).
fn track_namespace() -> impl Strategy<Value = Vec<String>> {
    vec(short_text(), 1..=32)
}

fn location() -> impl Strategy<Value = Location> {
    (varint(), varint()).prop_map(|(group_id, object_id)| Location {
        group_id,
        object_id,
    })
}

fn group_order() -> impl Strategy<Value = GroupOrder> {
    prop_oneof![
        Just(GroupOrder::Publisher),
        Just(GroupOrder::Ascending),
        Just(GroupOrder::Descending),
    ]
}

fn filter_type() -> impl Strategy<Value = FilterType> {
    prop_oneof![
        Just(FilterType::LargestObject),
        Just(FilterType::NextGroupStart),
        location().prop_map(|location| FilterType::AbsoluteStart { location }),
        (location(), varint()).prop_map(|(location, end_group)| FilterType::AbsoluteRange {
            location,
            end_group
        }),
    ]
}

fn token_value() -> impl Strategy<Value = Bytes> {
    vec(any::<u8>(), 0..64).prop_map(Bytes::from)
}

fn authorization_token() -> impl Strategy<Value = AuthorizationToken> {
    prop_oneof![
        Just(AuthorizationToken::Delete),
        (varint(), varint(), token_value()).prop_map(|(token_alias, token_type, token_value)| {
            AuthorizationToken::Register {
                token_alias,
                token_type,
                token_value,
            }
        }),
        varint().prop_map(|token_alias| AuthorizationToken::UseAlias { token_alias }),
        (varint(), token_value()).prop_map(|(token_type, token_value)| {
            AuthorizationToken::UseValue {
                token_type,
                token_value,
            }
        }),
    ]
}

fn authorization_tokens() -> impl Strategy<Value = Vec<AuthorizationToken>> {
    vec(authorization_token(), 0..4)
}

fn trace_context() -> impl Strategy<Value = Option<TraceContext>> {
    option::of(
        "00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}"
            .prop_filter_map("a well-formed traceparent", |traceparent| {
                TraceContext::parse(traceparent.as_bytes())
            }),
    )
}

prop_compose! {
    /// Draft-14 has no NEW_GROUP_REQUEST, so `new_group_request` is `None`.
    pub(crate) fn subscribe()(
        request_id in varint(),
        track_namespace in track_namespace(),
        track_name in short_text(),
        subscriber_priority in any::<u8>(),
        group_order in group_order(),
        forward in any::<bool>(),
        filter_type in filter_type(),
        authorization_tokens in authorization_tokens(),
        delivery_timeout in option::of(varint()),
        trace_context in trace_context(),
    ) -> Subscribe {
        Subscribe {
            request_id,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            forward,
            filter_type,
            authorization_tokens,
            delivery_timeout,
            new_group_request: None,
            trace_context,
        }
    }
}

prop_compose! {
    /// Draft-14 has no NEW_GROUP_REQUEST, so `new_group_request` is `None`.
    pub(crate) fn subscribe_update()(
        request_id in varint(),
        subscription_request_id in varint(),
        start_location in location(),
        end_group in varint(),
        subscriber_priority in any::<u8>(),
        forward in any::<bool>(),
        authorization_tokens in authorization_tokens(),
        delivery_timeout in option::of(varint()),
    ) -> SubscribeUpdate {
        SubscribeUpdate {
            request_id,
            subscription_request_id,
            start_location,
            end_group,
            subscriber_priority,
            forward,
            authorization_tokens,
            delivery_timeout,
            new_group_request: None,
        }
    }
}

prop_compose! {
    pub(crate) fn subscribe_namespace()(
        request_id in varint(),
        track_namespace_prefix in track_namespace(),
        authorization_token in authorization_tokens(),
        trace_context in trace_context(),
    ) -> SubscribeNamespace {
        SubscribeNamespace {
            request_id,
            track_namespace_prefix,
            authorization_token,
            trace_context,
        }
    }
}

prop_compose! {
    /// 256 characters of at most 4 bytes each keep the Error Reason within the
    /// 1,024-byte Reason Phrase limit (§1.4.3).
    pub(crate) fn publish_done()(
        request_id in varint(),
        status_code in varint(),
        stream_count in varint(),
        error_reason in vec(any::<char>(), 0..=256).prop_map(String::from_iter),
    ) -> PublishDone {
        PublishDone::new(request_id, status_code, stream_count, error_reason)
    }
}

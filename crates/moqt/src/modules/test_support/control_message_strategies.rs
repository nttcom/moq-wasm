//! `proptest` strategies that generate the control messages a peer may send:
//! draft-14 messages, and the model messages a draft-18 endpoint encodes.

use bytes::Bytes;
use proptest::{collection::vec, option, prelude::*};

use crate::modules::moqt::control_plane::{
    control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::{
            fetch::{Fetch, FetchParams},
            parameters::{
                authorization_token::AuthorizationToken,
                content_exists::ContentExists,
                filter_type::FilterType,
                group_order::GroupOrder,
                location::Location,
                trace_context::TraceContext,
                track_properties::{
                    DEFAULT_PUBLISHER_GROUP_ORDER, DEFAULT_PUBLISHER_PRIORITY, DYNAMIC_GROUPS,
                    IMMUTABLE_PROPERTIES, TrackProperties,
                },
            },
            publish::Publish,
            publish_done::PublishDone,
            publish_namespace::PublishNamespace,
            subscribe::Subscribe,
            subscribe_namespace::SubscribeNamespace,
            subscribe_update::SubscribeUpdate,
        },
    },
    error_codes::PublishDoneCode,
};
use crate::modules::test_support::object_strategies::vi64;

/// The integers of one draft: `draft14_integer` or `draft18_integer`.
type Integer = fn() -> BoxedStrategy<u64>;

fn draft14_integer() -> BoxedStrategy<u64> {
    varint().boxed()
}

fn draft18_integer() -> BoxedStrategy<u64> {
    vi64().boxed()
}

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

fn location(integer: Integer) -> impl Strategy<Value = Location> {
    (integer(), integer()).prop_map(|(group_id, object_id)| Location {
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
        location(draft14_integer).prop_map(|location| FilterType::AbsoluteStart { location }),
        (location(draft14_integer), varint()).prop_map(|(location, end_group)| {
            FilterType::AbsoluteRange {
                location,
                end_group,
            }
        }),
    ]
}

fn token_value() -> impl Strategy<Value = Bytes> {
    vec(any::<u8>(), 0..64).prop_map(Bytes::from)
}

fn authorization_token(integer: Integer) -> impl Strategy<Value = AuthorizationToken> {
    prop_oneof![
        Just(AuthorizationToken::Delete),
        (integer(), integer(), token_value()).prop_map(|(token_alias, token_type, token_value)| {
            AuthorizationToken::Register {
                token_alias,
                token_type,
                token_value,
            }
        }),
        integer().prop_map(|token_alias| AuthorizationToken::UseAlias { token_alias }),
        (integer(), token_value()).prop_map(|(token_type, token_value)| {
            AuthorizationToken::UseValue {
                token_type,
                token_value,
            }
        }),
    ]
}

fn authorization_tokens(integer: Integer) -> impl Strategy<Value = Vec<AuthorizationToken>> {
    vec(authorization_token(integer), 0..4)
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
        authorization_tokens in authorization_tokens(draft14_integer),
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
        start_location in location(draft14_integer),
        end_group in varint(),
        subscriber_priority in any::<u8>(),
        forward in any::<bool>(),
        authorization_tokens in authorization_tokens(draft14_integer),
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
        authorization_token in authorization_tokens(draft14_integer),
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

/// 1 to 32 fields of 1 to 16 characters: draft-18 refuses an empty field
/// (§2.4.1).
fn draft18_track_namespace() -> impl Strategy<Value = Vec<String>> {
    vec(
        vec(any::<char>(), 1..=16).prop_map(String::from_iter),
        1..=32,
    )
}

/// The end groups a range turns on, wrapping at the ends of `u64`: the one
/// before `start`, `start` itself and the one after, the largest Group ID,
/// or any.
fn group_near(start: u64) -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(u64::MAX),
        Just(start.wrapping_sub(1)),
        Just(start),
        Just(start.wrapping_add(1)),
        vi64(),
    ]
}

pub(crate) fn draft18_filter_type() -> impl Strategy<Value = FilterType> {
    prop_oneof![
        Just(FilterType::LargestObject),
        Just(FilterType::NextGroupStart),
        location(draft18_integer).prop_map(|location| FilterType::AbsoluteStart { location }),
        location(draft18_integer).prop_flat_map(|location| {
            group_near(location.group_id).prop_map(move |end_group| FilterType::AbsoluteRange {
                location,
                end_group,
            })
        }),
    ]
}

prop_compose! {
    pub(crate) fn draft18_subscribe()(
        request_id in vi64(),
        track_namespace in draft18_track_namespace(),
        track_name in short_text(),
        subscriber_priority in any::<u8>(),
        group_order in group_order(),
        forward in any::<bool>(),
        filter_type in draft18_filter_type(),
        authorization_tokens in authorization_tokens(draft18_integer),
        delivery_timeout in option::of(vi64()),
        new_group_request in option::of(vi64()),
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
            new_group_request,
            trace_context,
        }
    }
}

/// A SUBSCRIBE_UPDATE of the model, whose `end_group` is the last group plus
/// 1, or 0 for an open end: often 0, the start group (an empty range), the
/// one after it (one group) or the one after that.
pub(crate) fn draft18_subscribe_update() -> impl Strategy<Value = SubscribeUpdate> {
    (
        (vi64(), vi64(), location(draft18_integer)),
        any::<u8>(),
        any::<bool>(),
        authorization_tokens(draft18_integer),
        option::of(vi64()),
        option::of(vi64()),
    )
        .prop_flat_map(
            |(
                (request_id, subscription_request_id, start_location),
                subscriber_priority,
                forward,
                authorization_tokens,
                delivery_timeout,
                new_group_request,
            )| {
                let start = start_location.group_id;
                prop_oneof![
                    Just(0),
                    Just(start),
                    Just(start.wrapping_add(1)),
                    Just(start.wrapping_add(2)),
                    vi64(),
                ]
                .prop_map(move |end_group| SubscribeUpdate {
                    request_id,
                    subscription_request_id,
                    start_location,
                    end_group,
                    subscriber_priority,
                    forward,
                    authorization_tokens: authorization_tokens.clone(),
                    delivery_timeout,
                    new_group_request,
                })
            },
        )
}

prop_compose! {
    pub(crate) fn draft18_subscribe_namespace()(
        request_id in vi64(),
        track_namespace_prefix in draft18_track_namespace(),
        authorization_token in authorization_tokens(draft18_integer),
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

pub(crate) fn publish_done_code() -> impl Strategy<Value = PublishDoneCode> {
    proptest::sample::select(
        &[
            PublishDoneCode::InternalError,
            PublishDoneCode::Unauthorized,
            PublishDoneCode::TrackEnded,
            PublishDoneCode::SubscriptionEnded,
            PublishDoneCode::GoingAway,
            PublishDoneCode::Expired,
            PublishDoneCode::TooFarBehind,
            PublishDoneCode::MalformedTrack,
            PublishDoneCode::UpdateFailed,
            PublishDoneCode::ExcessiveLoad,
        ][..],
    )
}

/// Short text, or text whose 1,024-byte Reason Phrase limit (§1.4.4) falls
/// at the start, middle or end of a character: ASCII up to just below the
/// limit, then a few characters of any width.
pub(crate) fn reason_phrase() -> impl Strategy<Value = String> {
    let character = || prop_oneof![any::<char>(), proptest::char::range(' ', '~')];
    prop_oneof![
        vec(character(), 0..=64).prop_map(String::from_iter),
        (1016..=1024usize, vec(character(), 0..=8))
            .prop_map(|(ascii, tail)| { "a".repeat(ascii) + &String::from_iter(tail) }),
    ]
}

fn fetch_params() -> impl Strategy<Value = FetchParams> {
    prop_oneof![
        (
            draft18_track_namespace(),
            short_text(),
            location(draft18_integer),
            location(draft18_integer),
        )
            .prop_map(
                |(track_namespace, track_name, start_location, end_location)| {
                    FetchParams::Standalone {
                        track_namespace,
                        track_name,
                        start_location,
                        end_location,
                    }
                }
            ),
        (vi64(), vi64()).prop_map(|(joining_request_id, joining_start)| {
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            }
        }),
        (vi64(), vi64()).prop_map(|(joining_request_id, joining_start)| {
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            }
        }),
    ]
}

prop_compose! {
    pub(crate) fn draft18_fetch()(
        request_id in vi64(),
        subscriber_priority in any::<u8>(),
        group_order in group_order(),
        fetch_params in fetch_params(),
        authorization_tokens in authorization_tokens(draft18_integer),
        fill_timeout in option::of(vi64()),
        trace_context in trace_context(),
    ) -> Fetch {
        Fetch {
            request_id,
            subscriber_priority,
            group_order,
            fetch_params,
            authorization_tokens,
            fill_timeout,
            trace_context,
        }
    }
}

/// Track Properties a draft-18 receiver accepts (§12): DEFAULT PUBLISHER
/// PRIORITY up to 255, DEFAULT PUBLISHER GROUP ORDER 1 or 2 and
/// DYNAMIC_GROUPS 0 or 1, often at the edges, among properties of other
/// types with any value.
/// Immutable Properties are left out: their contents hold the same checks.
fn track_properties() -> impl Strategy<Value = TrackProperties> {
    let checked = [
        IMMUTABLE_PROPERTIES,
        DEFAULT_PUBLISHER_PRIORITY,
        DEFAULT_PUBLISHER_GROUP_ORDER,
        DYNAMIC_GROUPS,
    ];
    let other = prop_oneof![0..64u64, vi64()]
        .prop_filter("a type without a value check", move |key| {
            !checked.contains(key)
        })
        .prop_flat_map(|key| {
            let value = if key % 2 == 0 {
                vi64().prop_map(VariantType::Even).boxed()
            } else {
                token_value().prop_map(VariantType::Odd).boxed()
            };
            value.prop_map(move |value| KeyValuePair { key, value })
        });
    let even = |key: u64, values: BoxedStrategy<u64>| {
        option::of(values.prop_map(move |value| KeyValuePair {
            key,
            value: VariantType::Even(value),
        }))
    };
    (
        vec(other, 0..4),
        even(
            DEFAULT_PUBLISHER_PRIORITY,
            prop_oneof![Just(0u64), Just(255), 0..=255u64].boxed(),
        ),
        even(DEFAULT_PUBLISHER_GROUP_ORDER, (1..=2u64).boxed()),
        even(DYNAMIC_GROUPS, (0..=1u64).boxed()),
    )
        .prop_map(|(mut pairs, priority, group_order, dynamic_groups)| {
            pairs.extend(
                [priority, group_order, dynamic_groups]
                    .into_iter()
                    .flatten(),
            );
            TrackProperties::from_pairs(pairs)
        })
}

fn content_exists() -> impl Strategy<Value = ContentExists> {
    prop_oneof![
        Just(ContentExists::False),
        location(draft18_integer).prop_map(|location| ContentExists::True { location }),
    ]
}

prop_compose! {
    pub(crate) fn draft18_publish()(
        request_id in vi64(),
        track_namespace_tuple in draft18_track_namespace(),
        track_name in short_text(),
        track_alias in vi64(),
        content_exists in content_exists(),
        forward in any::<bool>(),
        authorization_tokens in authorization_tokens(draft18_integer),
        track_properties in track_properties(),
        trace_context in trace_context(),
    ) -> Publish {
        Publish {
            request_id,
            track_namespace_tuple,
            track_name,
            track_alias,
            content_exists,
            forward,
            authorization_tokens,
            track_properties,
            trace_context,
        }
    }
}

/// Integers of either draft: draft-14 variable-length integers stop below
/// 2^62 (draft-14 §1.4.1), draft-18 ones reach `u64::MAX` (§1.4.1), so 0,
/// the values on either side of 2^62 and the largest come often.
fn cross_draft_integer() -> BoxedStrategy<u64> {
    const DRAFT_EDGES: [u64; 5] = [0, (1 << 62) - 1, 1 << 62, u64::MAX - 1, u64::MAX];
    prop_oneof![varint(), vi64(), proptest::sample::select(&DRAFT_EDGES[..])].boxed()
}

/// Set by §2.4.1 of draft-14 and draft-18.
const MAX_NAMESPACE_FIELDS: usize = 32;
const MAX_NAME_BYTES: usize = 4096;

/// Namespaces on both sides of each draft's limits (§2.4.1), weighted to
/// 0 fields (draft-18 only), 1 and 32 (both) and 33 (neither), to one
/// emptied field (draft-14 only), and to 4,095, 4,096 or 4,097 bytes.
/// At most one field is emptied: draft-18 refuses any empty field, so
/// more would hide the field counts.
fn cross_draft_track_namespace() -> impl Strategy<Value = Vec<String>> {
    prop_oneof![
        Just(0usize),
        Just(1),
        Just(MAX_NAMESPACE_FIELDS),
        Just(MAX_NAMESPACE_FIELDS + 1),
        0..=MAX_NAMESPACE_FIELDS + 1,
    ]
    .prop_flat_map(|count| {
        (
            vec(
                vec(any::<char>(), 1..=16).prop_map(String::from_iter),
                count,
            ),
            option::of(any::<proptest::sample::Index>()),
            option::of((
                any::<proptest::sample::Index>(),
                MAX_NAME_BYTES - 1..=MAX_NAME_BYTES + 1,
            )),
        )
    })
    .prop_map(|(mut namespace, emptied, filled)| {
        if namespace.is_empty() {
            return namespace;
        }
        if let Some(index) = emptied {
            let emptied = index.index(namespace.len());
            namespace[emptied].clear();
        }
        if let Some((index, total)) = filled {
            let used: usize = namespace.iter().map(String::len).sum();
            let filled = index.index(namespace.len());
            namespace[filled].push_str(&"n".repeat(total.saturating_sub(used)));
        }
        namespace
    })
}

fn cross_draft_full_track_name() -> impl Strategy<Value = (Vec<String>, String)> {
    cross_draft_track_namespace().prop_flat_map(|namespace| {
        let used: usize = namespace.iter().map(String::len).sum();
        let at_limit =
            move |offset: usize| "n".repeat((MAX_NAME_BYTES - 1 + offset).saturating_sub(used));
        let name = prop_oneof![
            short_text().boxed(),
            (0..=2usize).prop_map(at_limit).boxed(),
        ];
        (Just(namespace), name)
    })
}

fn cross_draft_filter_type() -> impl Strategy<Value = FilterType> {
    prop_oneof![
        Just(FilterType::LargestObject),
        Just(FilterType::NextGroupStart),
        location(cross_draft_integer).prop_map(|location| FilterType::AbsoluteStart { location }),
        location(cross_draft_integer).prop_flat_map(|location| {
            prop_oneof![group_near(location.group_id).boxed(), cross_draft_integer()].prop_map(
                move |end_group| FilterType::AbsoluteRange {
                    location,
                    end_group,
                },
            )
        }),
    ]
}

prop_compose! {
    pub(crate) fn cross_draft_subscribe()(
        request_id in cross_draft_integer(),
        (track_namespace, track_name) in cross_draft_full_track_name(),
        subscriber_priority in any::<u8>(),
        group_order in group_order(),
        forward in any::<bool>(),
        filter_type in cross_draft_filter_type(),
        authorization_tokens in authorization_tokens(cross_draft_integer),
        delivery_timeout in option::of(cross_draft_integer()),
        new_group_request in option::of(cross_draft_integer()),
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
            new_group_request,
            trace_context,
        }
    }
}

prop_compose! {
    pub(crate) fn cross_draft_subscribe_namespace()(
        request_id in cross_draft_integer(),
        track_namespace_prefix in cross_draft_track_namespace(),
        authorization_token in authorization_tokens(cross_draft_integer),
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
    pub(crate) fn cross_draft_publish_namespace()(
        request_id in cross_draft_integer(),
        track_namespace in cross_draft_track_namespace(),
        authorization_token in authorization_tokens(cross_draft_integer),
        trace_context in trace_context(),
    ) -> PublishNamespace {
        PublishNamespace {
            request_id,
            track_namespace,
            authorization_token,
            trace_context,
        }
    }
}

/// Track Properties of either draft: draft-18's, then often the draft-14
/// PUBLISH fields the model keeps there (draft-14 §9.13: Group Order,
/// DELIVERY_TIMEOUT, MAX_CACHE_DURATION), set to any value.
fn cross_draft_track_properties() -> impl Strategy<Value = TrackProperties> {
    (
        track_properties(),
        option::of(group_order()),
        option::of(option::of(cross_draft_integer())),
        option::of(option::of(cross_draft_integer())),
    )
        .prop_map(
            |(mut properties, group_order, delivery_timeout, max_cache_duration)| {
                if let Some(group_order) = group_order {
                    properties.set_group_order(Some(group_order));
                }
                if let Some(delivery_timeout) = delivery_timeout {
                    properties.set_delivery_timeout(delivery_timeout);
                }
                if let Some(max_cache_duration) = max_cache_duration {
                    properties.set_max_cache_duration(max_cache_duration);
                }
                properties
            },
        )
}

prop_compose! {
    pub(crate) fn cross_draft_publish()(
        request_id in cross_draft_integer(),
        (track_namespace_tuple, track_name) in cross_draft_full_track_name(),
        track_alias in cross_draft_integer(),
        content_exists in prop_oneof![
            Just(ContentExists::False),
            location(cross_draft_integer).prop_map(|location| ContentExists::True { location }),
        ],
        forward in any::<bool>(),
        authorization_tokens in authorization_tokens(cross_draft_integer),
        track_properties in cross_draft_track_properties(),
        trace_context in trace_context(),
    ) -> Publish {
        Publish {
            request_id,
            track_namespace_tuple,
            track_name,
            track_alias,
            content_exists,
            forward,
            authorization_tokens,
            track_properties,
            trace_context,
        }
    }
}

/// Reason phrases within and just past the 1,024-byte limit both drafts
/// share (draft-14 §1.4.3, draft-18 §1.4.4): also 1,020 to 1,025 ASCII
/// bytes followed by nothing or one character of 2, 3 or 4 bytes, so the
/// limit falls before, inside and after a multibyte character.
fn cross_draft_reason_phrase() -> impl Strategy<Value = String> {
    prop_oneof![
        reason_phrase(),
        (
            1020..=1025usize,
            proptest::sample::select(&["", "é", "あ", "😀"][..]),
        )
            .prop_map(|(ascii, tail)| "r".repeat(ascii) + tail),
    ]
}

/// A PUBLISH_DONE of the model: its code, Stream Count and reason phrase.
pub(crate) fn cross_draft_publish_done() -> impl Strategy<Value = (PublishDoneCode, u64, String)> {
    (
        publish_done_code(),
        cross_draft_integer(),
        cross_draft_reason_phrase(),
    )
}

fn cross_draft_fetch_params() -> impl Strategy<Value = FetchParams> {
    prop_oneof![
        (
            cross_draft_full_track_name(),
            location(cross_draft_integer),
            location(cross_draft_integer),
        )
            .prop_map(
                |((track_namespace, track_name), start_location, end_location)| {
                    FetchParams::Standalone {
                        track_namespace,
                        track_name,
                        start_location,
                        end_location,
                    }
                }
            ),
        (cross_draft_integer(), cross_draft_integer()).prop_map(
            |(joining_request_id, joining_start)| FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            }
        ),
        (cross_draft_integer(), cross_draft_integer()).prop_map(
            |(joining_request_id, joining_start)| FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            }
        ),
    ]
}

prop_compose! {
    pub(crate) fn cross_draft_fetch()(
        request_id in cross_draft_integer(),
        subscriber_priority in any::<u8>(),
        group_order in group_order(),
        fetch_params in cross_draft_fetch_params(),
        authorization_tokens in authorization_tokens(cross_draft_integer),
        fill_timeout in option::of(cross_draft_integer()),
        trace_context in trace_context(),
    ) -> Fetch {
        Fetch {
            request_id,
            subscriber_priority,
            group_order,
            fetch_params,
            authorization_tokens,
            fill_timeout,
            trace_context,
        }
    }
}

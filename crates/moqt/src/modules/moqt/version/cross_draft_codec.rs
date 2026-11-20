//! A relay decodes a request from a peer on one draft into the shared model
//! and encodes it again for a peer on the other. These properties state what
//! each hop of that path keeps: a draft's encoder fails exactly when the
//! message holds something the draft cannot express, and otherwise its
//! receiver reads back the message, less only what the draft does not carry.
//! So a value crosses from draft-18 to draft-14 and back with its meaning,
//! or the request fails; it is never changed silently.

use std::fmt::Debug;
use std::io::Cursor;

use bytes::{Bytes, BytesMut};
use proptest::prelude::*;

use super::draft14::error_codes::{publish_done_code_from_wire, publish_done_code_to_wire};
use super::draft18::messages::{
    Message, decode_message, encode_fetch, encode_publish, encode_publish_done,
    encode_publish_namespace, encode_subscribe, encode_subscribe_namespace, encode_track_status,
    take_message,
};
use crate::modules::extensions::name_reader::MAX_REASON_PHRASE_LENGTH;
use crate::modules::moqt::control_plane::control_messages::messages::{
    fetch::{Fetch, FetchParams},
    parameters::{
        authorization_token::AuthorizationToken, content_exists::ContentExists,
        filter_type::FilterType, group_order::GroupOrder, location::Location,
        track_properties::TrackProperties,
    },
    publish::Publish,
    publish_done::PublishDone,
    publish_namespace::PublishNamespace,
    subscribe::Subscribe,
    subscribe_namespace::SubscribeNamespace,
};
use crate::modules::moqt::control_plane::error_codes::PublishDoneCode;
use crate::modules::test_support::control_message_strategies::{
    cross_draft_fetch, cross_draft_publish, cross_draft_publish_done,
    cross_draft_publish_namespace, cross_draft_subscribe, cross_draft_subscribe_namespace,
};

/// A model PUBLISH_DONE: its code, Stream Count and reason phrase.
type Done = (PublishDoneCode, u64, String);

/// How one draft carries a model message `M`.
struct Draft<M> {
    name: &'static str,
    encode: fn(&M) -> anyhow::Result<Bytes>,
    /// The receiver's decoder, which reads one whole message.
    decode: fn(Bytes) -> anyhow::Result<M>,
    /// Whether the draft can express the message, so that encoding succeeds.
    expresses: fn(&M) -> bool,
    /// What the receiver reads of the message.
    carries: fn(&M) -> M,
}

/// Sends `message` over `draft`: the encoder fails exactly when the draft
/// cannot express it, and otherwise the receiver reads what the draft
/// carries of it, which is returned.
fn hop<M: PartialEq + Debug>(draft: &Draft<M>, message: &M) -> Result<Option<M>, TestCaseError> {
    let encoded = (draft.encode)(message);
    prop_assert_eq!(
        encoded.is_ok(),
        (draft.expresses)(message),
        "{} encoded {:?} into {:?}",
        draft.name,
        message,
        encoded,
    );
    let Ok(bytes) = encoded else {
        return Ok(None);
    };
    let received = (draft.decode)(bytes)
        .map_err(|error| TestCaseError::fail(format!("{} receiver: {error:#}", draft.name)))?;
    prop_assert_eq!(&received, &(draft.carries)(message), "over {}", draft.name);
    Ok(Some(received))
}

/// A peer of each draft sends `message`, and a relay forwards what it
/// received to a peer of the other draft.
fn crosses_both_ways<M: PartialEq + Debug>(
    message: &M,
    draft14: Draft<M>,
    draft18: Draft<M>,
) -> Result<(), TestCaseError> {
    for (from, to) in [(&draft14, &draft18), (&draft18, &draft14)] {
        if let Some(received) = hop(from, message)? {
            hop(to, &received)?;
        }
    }
    Ok(())
}

/// Draft-14 variable-length integers stop below 2^62 (§1.4.1).
fn draft14_expresses(integers: &[u64]) -> bool {
    integers.iter().all(|integer| *integer < 1 << 62)
}

/// Set by §2.4.1 of draft-14 and draft-18.
const MAX_NAMESPACE_FIELDS: usize = 32;
const MAX_NAME_BYTES: usize = 4096;

fn namespace_bytes(namespace: &[String]) -> usize {
    namespace.iter().map(String::len).sum()
}

fn full_track_name_fits(namespace: &[String], name: &str) -> bool {
    namespace_bytes(namespace) + name.len() <= MAX_NAME_BYTES
}

/// Draft-14 §2.4.1 bounds only the Full Track Name to 4,096 bytes, but a
/// namespace over that leaves no room for a track name, so it is refused
/// on its own as in draft-18.
fn draft14_expresses_namespace(namespace: &[String]) -> bool {
    (1..=MAX_NAMESPACE_FIELDS).contains(&namespace.len())
        && namespace_bytes(namespace) <= MAX_NAME_BYTES
}

/// Bounds the namespace as §2.4.1 does, and the model holds a DELETE
/// token without the alias draft-18 sends with it (§10.2.2).
fn draft18_expresses(namespace: &[String], tokens: &[AuthorizationToken]) -> bool {
    namespace.len() <= MAX_NAMESPACE_FIELDS
        && !namespace.iter().any(String::is_empty)
        && namespace_bytes(namespace) <= MAX_NAME_BYTES
        && !tokens.contains(&AuthorizationToken::Delete)
}

/// The integers a draft-14 AUTHORIZATION TOKEN parameter writes (§9.2.1.1).
fn token_integers(tokens: &[AuthorizationToken]) -> Vec<u64> {
    tokens
        .iter()
        .flat_map(|token| match token {
            AuthorizationToken::Delete => vec![],
            AuthorizationToken::Register {
                token_alias,
                token_type,
                ..
            } => vec![*token_alias, *token_type],
            AuthorizationToken::UseAlias { token_alias } => vec![*token_alias],
            AuthorizationToken::UseValue { token_type, .. } => vec![*token_type],
        })
        .collect()
}

fn location_integers(location: &Location) -> [u64; 2] {
    [location.group_id, location.object_id]
}

/// The longest prefix of `reason` within 1,024 bytes that ends on a
/// character (draft-14 §1.4.3, draft-18 §1.4.4).
fn cut_reason(reason: &str) -> String {
    let end = reason
        .char_indices()
        .map(|(start, character)| start + character.len_utf8())
        .take_while(|end| *end <= MAX_REASON_PHRASE_LENGTH as usize)
        .last()
        .unwrap_or(0);
    reason[..end].to_string()
}

/// Reads one draft-14 message payload that fills its Message Length.
fn draft14_decode<M>(
    decode: fn(&mut Cursor<&[u8]>) -> anyhow::Result<M>,
    bytes: &[u8],
) -> anyhow::Result<M> {
    let mut cursor = Cursor::new(bytes);
    let message = decode(&mut cursor)?;
    anyhow::ensure!(
        cursor.position() == bytes.len() as u64,
        "bytes after the message"
    );
    Ok(message)
}

/// Reads one framed draft-18 message that fills `bytes`.
fn draft18_decode(bytes: Bytes) -> anyhow::Result<Message> {
    let mut framed = BytesMut::from(&bytes[..]);
    let (message_type, payload) = take_message(&mut framed).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(framed.is_empty(), "bytes after the message");
    decode_message(message_type, payload).map_err(anyhow::Error::msg)
}

fn filter_integers(filter: &FilterType) -> Vec<u64> {
    match filter {
        FilterType::LargestObject | FilterType::NextGroupStart => vec![],
        FilterType::AbsoluteStart { location } => location_integers(location).to_vec(),
        FilterType::AbsoluteRange {
            location,
            end_group,
        } => [location.group_id, location.object_id, *end_group].to_vec(),
    }
}

fn draft14_expresses_subscribe(subscribe: &Subscribe) -> bool {
    let mut integers = vec![subscribe.request_id];
    integers.extend(filter_integers(&subscribe.filter_type));
    integers.extend(token_integers(&subscribe.authorization_tokens));
    integers.extend(subscribe.delivery_timeout);
    draft14_expresses_namespace(&subscribe.track_namespace)
        && full_track_name_fits(&subscribe.track_namespace, &subscribe.track_name)
        && draft14_expresses(&integers)
}

/// Draft-14 lays TRACK_STATUS out as SUBSCRIBE (§9.17) and has no
/// NEW_GROUP_REQUEST.
fn draft14_subscribe() -> Draft<Subscribe> {
    Draft {
        name: "draft-14",
        encode: |subscribe| Ok(subscribe.encode()?.freeze()),
        decode: |bytes| draft14_decode(Subscribe::decode, &bytes),
        expresses: draft14_expresses_subscribe,
        carries: |subscribe| Subscribe {
            new_group_request: None,
            ..subscribe.clone()
        },
    }
}

fn draft18_subscribe() -> Draft<Subscribe> {
    Draft {
        name: "draft-18",
        encode: |subscribe| Ok(encode_subscribe(subscribe)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::Subscribe(subscribe) => Ok(subscribe),
            message => anyhow::bail!("decoded {message:?}"),
        },
        // A range ending before its start group has no End Group Delta (§5.1.2).
        expresses: |subscribe| {
            draft18_expresses(&subscribe.track_namespace, &subscribe.authorization_tokens)
                && full_track_name_fits(&subscribe.track_namespace, &subscribe.track_name)
                && !matches!(
                    subscribe.filter_type,
                    FilterType::AbsoluteRange { location, end_group } if end_group < location.group_id
                )
        },
        carries: Subscribe::clone,
    }
}

/// §10.14: draft-18 TRACK_STATUS carries the Full Track Name and the
/// authorization tokens; the receiver reads the defaults of the rest
/// (§10.2.4 SUBSCRIBER_PRIORITY 128, §10.2.7 FORWARD 1, §5.1.2 an absent
/// filter is AbsoluteStart {0, 0}).
fn draft18_track_status() -> Draft<Subscribe> {
    Draft {
        name: "draft-18",
        encode: |track_status| Ok(encode_track_status(track_status)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::TrackStatus(track_status) => Ok(track_status),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |track_status| {
            draft18_expresses(
                &track_status.track_namespace,
                &track_status.authorization_tokens,
            ) && full_track_name_fits(&track_status.track_namespace, &track_status.track_name)
        },
        carries: |track_status| Subscribe {
            request_id: track_status.request_id,
            track_namespace: track_status.track_namespace.clone(),
            track_name: track_status.track_name.clone(),
            subscriber_priority: 128,
            group_order: GroupOrder::Publisher,
            forward: true,
            filter_type: FilterType::AbsoluteStart {
                location: Location {
                    group_id: 0,
                    object_id: 0,
                },
            },
            authorization_tokens: track_status.authorization_tokens.clone(),
            delivery_timeout: None,
            new_group_request: None,
            trace_context: None,
        },
    }
}

fn draft14_subscribe_namespace() -> Draft<SubscribeNamespace> {
    Draft {
        name: "draft-14",
        encode: |message| Ok(message.encode()?.freeze()),
        decode: |bytes| draft14_decode(SubscribeNamespace::decode, &bytes),
        expresses: |message| {
            let mut integers = vec![message.request_id];
            integers.extend(token_integers(&message.authorization_token));
            draft14_expresses_namespace(&message.track_namespace_prefix)
                && draft14_expresses(&integers)
        },
        carries: SubscribeNamespace::clone,
    }
}

fn draft18_subscribe_namespace() -> Draft<SubscribeNamespace> {
    Draft {
        name: "draft-18",
        encode: |message| Ok(encode_subscribe_namespace(message)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::SubscribeNamespace(message) => Ok(message),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |message| {
            draft18_expresses(
                &message.track_namespace_prefix,
                &message.authorization_token,
            )
        },
        carries: SubscribeNamespace::clone,
    }
}

fn draft14_publish_namespace() -> Draft<PublishNamespace> {
    Draft {
        name: "draft-14",
        encode: |message| Ok(message.encode()?.freeze()),
        decode: |bytes| draft14_decode(PublishNamespace::decode, &bytes),
        expresses: |message| {
            let mut integers = vec![message.request_id];
            integers.extend(token_integers(&message.authorization_token));
            draft14_expresses_namespace(&message.track_namespace) && draft14_expresses(&integers)
        },
        carries: PublishNamespace::clone,
    }
}

fn draft18_publish_namespace() -> Draft<PublishNamespace> {
    Draft {
        name: "draft-18",
        encode: |message| Ok(encode_publish_namespace(message)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::PublishNamespace(message) => Ok(message),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |message| {
            draft18_expresses(&message.track_namespace, &message.authorization_token)
        },
        carries: PublishNamespace::clone,
    }
}

fn draft14_expresses_publish(publish: &Publish) -> bool {
    let mut integers = vec![publish.request_id, publish.track_alias];
    if let ContentExists::True { location } = &publish.content_exists {
        integers.extend(location_integers(location));
    }
    integers.extend(token_integers(&publish.authorization_tokens));
    integers.extend(publish.track_properties.delivery_timeout());
    integers.extend(publish.track_properties.max_cache_duration());
    draft14_expresses_namespace(&publish.track_namespace_tuple)
        && full_track_name_fits(&publish.track_namespace_tuple, &publish.track_name)
        && draft14_expresses(&integers)
}

/// Of the Track Properties, a draft-14 PUBLISH carries the Group Order field,
/// Ascending when none is set, and the DELIVERY_TIMEOUT and
/// MAX_CACHE_DURATION parameters (§9.13).
fn draft14_publish() -> Draft<Publish> {
    Draft {
        name: "draft-14",
        encode: |publish| Ok(publish.encode()?.freeze()),
        decode: |bytes| draft14_decode(Publish::decode, &bytes),
        expresses: draft14_expresses_publish,
        carries: |publish| {
            let sent = &publish.track_properties;
            let mut track_properties = TrackProperties::default();
            track_properties
                .set_group_order(Some(sent.group_order().unwrap_or(GroupOrder::Ascending)));
            track_properties.set_delivery_timeout(sent.delivery_timeout());
            track_properties.set_max_cache_duration(sent.max_cache_duration());
            Publish {
                track_properties,
                ..publish.clone()
            }
        },
    }
}

fn draft18_publish() -> Draft<Publish> {
    Draft {
        name: "draft-18",
        encode: |publish| Ok(encode_publish(publish)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::Publish { publish } => Ok(publish),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |publish| {
            draft18_expresses(
                &publish.track_namespace_tuple,
                &publish.authorization_tokens,
            ) && full_track_name_fits(&publish.track_namespace_tuple, &publish.track_name)
        },
        carries: Publish::clone,
    }
}

/// Both drafts cut the reason phrase to 1,024 bytes at a character.
/// Draft-14 defines no UPDATE_FAILED or EXCESSIVE_LOAD (§13.1.3) and sends
/// INTERNAL_ERROR for them.
fn draft14_publish_done() -> Draft<Done> {
    Draft {
        name: "draft-14",
        encode: |(code, stream_count, reason)| {
            let done = PublishDone::new(
                0,
                publish_done_code_to_wire(*code),
                *stream_count,
                reason.clone(),
            );
            Ok(done.encode()?.freeze())
        },
        decode: |bytes| {
            let done = draft14_decode(PublishDone::decode, &bytes)?;
            let code = publish_done_code_from_wire(done.status_code)
                .ok_or_else(|| anyhow::anyhow!("unknown status code {}", done.status_code))?;
            Ok((code, done.stream_count, done.error_reason))
        },
        expresses: |(_, stream_count, _)| draft14_expresses(&[*stream_count]),
        carries: |(code, stream_count, reason)| {
            let code = match code {
                PublishDoneCode::UpdateFailed | PublishDoneCode::ExcessiveLoad => {
                    PublishDoneCode::InternalError
                }
                code => *code,
            };
            (code, *stream_count, cut_reason(reason))
        },
    }
}

fn draft18_publish_done() -> Draft<Done> {
    Draft {
        name: "draft-18",
        encode: |(code, stream_count, reason)| {
            Ok(encode_publish_done(*code, *stream_count, reason)?.freeze())
        },
        decode: |bytes| match draft18_decode(bytes)? {
            Message::PublishDone(done) => Ok((done.code, done.stream_count, done.reason)),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |_| true,
        carries: |(code, stream_count, reason)| (*code, *stream_count, cut_reason(reason)),
    }
}

fn fetch_name(fetch: &Fetch) -> Option<(&[String], &str)> {
    match &fetch.fetch_params {
        FetchParams::Standalone {
            track_namespace,
            track_name,
            ..
        } => Some((track_namespace, track_name)),
        FetchParams::RelativeJoining { .. } | FetchParams::AbsoluteJoining { .. } => None,
    }
}

fn draft14_expresses_fetch(fetch: &Fetch) -> bool {
    let mut integers = vec![fetch.request_id];
    match &fetch.fetch_params {
        FetchParams::Standalone {
            start_location,
            end_location,
            ..
        } => {
            integers.extend(location_integers(start_location));
            integers.extend(location_integers(end_location));
        }
        FetchParams::RelativeJoining {
            joining_request_id,
            joining_start,
        }
        | FetchParams::AbsoluteJoining {
            joining_request_id,
            joining_start,
        } => integers.extend([*joining_request_id, *joining_start]),
    }
    integers.extend(token_integers(&fetch.authorization_tokens));
    fetch_name(fetch).is_none_or(|(namespace, name)| {
        draft14_expresses_namespace(namespace) && full_track_name_fits(namespace, name)
    }) && draft14_expresses(&integers)
}

/// Draft-14 FETCH has no fill timeout.
fn draft14_fetch() -> Draft<Fetch> {
    Draft {
        name: "draft-14",
        encode: |fetch| Ok(fetch.encode()?.freeze()),
        decode: |bytes| draft14_decode(Fetch::decode, &bytes),
        expresses: draft14_expresses_fetch,
        carries: |fetch| Fetch {
            fill_timeout: None,
            ..fetch.clone()
        },
    }
}

/// §10.2.8: an omitted GROUP_ORDER in FETCH is Ascending, and draft-18 has
/// no value for the publisher's order.
fn draft18_fetch() -> Draft<Fetch> {
    Draft {
        name: "draft-18",
        encode: |fetch| Ok(encode_fetch(fetch)?.freeze()),
        decode: |bytes| match draft18_decode(bytes)? {
            Message::Fetch(fetch) => Ok(fetch),
            message => anyhow::bail!("decoded {message:?}"),
        },
        expresses: |fetch| {
            let (namespace, name) = fetch_name(fetch).unwrap_or_default();
            draft18_expresses(namespace, &fetch.authorization_tokens)
                && full_track_name_fits(namespace, name)
        },
        carries: |fetch| Fetch {
            group_order: match fetch.group_order {
                GroupOrder::Publisher => GroupOrder::Ascending,
                order => order,
            },
            ..fetch.clone()
        },
    }
}

proptest! {
    // Most messages hold one value a draft cannot express, so few cross both
    // hops at the default 256 cases.
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn a_subscribe_crosses_between_drafts(message in cross_draft_subscribe()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_subscribe(), draft18_subscribe())?;
    }

    #[test]
    fn a_track_status_crosses_between_drafts(message in cross_draft_subscribe()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_subscribe(), draft18_track_status())?;
    }

    #[test]
    fn a_subscribe_namespace_crosses_between_drafts(message in cross_draft_subscribe_namespace()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_subscribe_namespace(), draft18_subscribe_namespace())?;
    }

    #[test]
    fn a_publish_namespace_crosses_between_drafts(message in cross_draft_publish_namespace()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_publish_namespace(), draft18_publish_namespace())?;
    }

    #[test]
    fn a_publish_crosses_between_drafts(message in cross_draft_publish()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_publish(), draft18_publish())?;
    }

    #[test]
    fn a_publish_done_crosses_between_drafts(message in cross_draft_publish_done()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_publish_done(), draft18_publish_done())?;
    }

    #[test]
    fn a_fetch_crosses_between_drafts(message in cross_draft_fetch()) {
        // Act / Assert
        crosses_both_ways(&message, draft14_fetch(), draft18_fetch())?;
    }
}

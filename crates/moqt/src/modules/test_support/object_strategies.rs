//! `proptest` strategies that generate the objects of a subgroup stream or a
//! FETCH response, for properties of the data plane codecs.

use bytes::Bytes;
use proptest::{collection::vec, prelude::*};

use crate::modules::moqt::{
    control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{group_order::GroupOrder, location::Location},
    },
    data_plane::object::{
        canonical_object::{ForwardingPreference, Object, SubgroupStart},
        extension_headers::ExtensionHeaders,
        fetch::{FetchObject, FetchObjectField},
        object_status::ObjectStatus,
        subgroup::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
    },
};
use crate::modules::test_support::control_message_strategies::varint;

/// Any `u64`, each of the nine draft-18 variable-length integer lengths
/// (§1.4.1) equally likely, plus the values on either side of each length
/// boundary.
pub(crate) fn vi64() -> impl Strategy<Value = u64> {
    let length = 1..=9u32;
    let edges = (1..=8u32)
        .flat_map(|length| [(1u64 << (7 * length)) - 1, 1 << (7 * length)])
        .chain([u64::MAX])
        .collect::<Vec<_>>();
    prop_oneof![
        length.prop_flat_map(|length| match length {
            1 => (0..1u64 << 7).boxed(),
            9 => (1u64 << 56..=u64::MAX).boxed(),
            length => ((1u64 << (7 * (length - 1)))..(1 << (7 * length))).boxed(),
        }),
        proptest::sample::select(edges),
    ]
}

/// The distance to the next object or group less 1: often 0 to 3, where
/// the codecs omit or shorten a delta, and otherwise anything below 2^20.
fn gap() -> impl Strategy<Value = u64> {
    prop_oneof![0..4u64, 0..1u64 << 20]
}

fn payload() -> impl Strategy<Value = Bytes> {
    vec(any::<u8>(), 0..64).prop_map(Bytes::from)
}

/// A key-value pair whose value follows the key's parity: from `integer` for
/// an even key, bytes for an odd one.
fn pair<S: Strategy<Value = u64> + 'static>(
    key: impl Strategy<Value = u64>,
    integer: fn() -> S,
) -> impl Strategy<Value = KeyValuePair> {
    key.prop_flat_map(move |key| {
        let value = if key % 2 == 0 {
            integer().prop_map(VariantType::Even).boxed()
        } else {
            payload().prop_map(VariantType::Odd).boxed()
        };
        value.prop_map(move |value| KeyValuePair { key, value })
    })
}

/// Keys in any order, repeats included: draft-14 does not delta-encode them,
/// and a relay forwards unsupported headers unmodified (§10.2.1.2).
fn draft14_extension_headers() -> impl Strategy<Value = ExtensionHeaders> {
    vec(pair(varint(), varint), 0..4).prop_map(ExtensionHeaders::new)
}

/// Draft-18 delta-encodes property types, so the encoder takes them in
/// ascending order (§1.4.3); a repeated type, which small types make
/// likely, is a delta of 0.
fn draft18_properties() -> impl Strategy<Value = ExtensionHeaders> {
    vec(pair(prop_oneof![0..8u64, vi64()], vi64), 0..4).prop_map(|mut pairs| {
        pairs.sort_by_key(|pair| pair.key);
        ExtensionHeaders::new(pairs)
    })
}

/// A draft-14 subgroup stream: its header and objects of any type the header
/// allows.
pub(crate) fn draft14_subgroup() -> impl Strategy<Value = (SubgroupHeader, Vec<SubgroupObjectField>)>
{
    let subgroup_id = prop_oneof![
        Just(SubgroupId::None),
        Just(SubgroupId::FirstObjectIdDelta),
        varint().prop_map(SubgroupId::Value),
    ];
    (
        varint(),
        varint(),
        subgroup_id,
        any::<u8>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_flat_map(
            |(track_alias, group_id, subgroup_id, priority, has_extensions, end_of_group)| {
                let header = SubgroupHeader::new(
                    track_alias,
                    group_id,
                    subgroup_id,
                    priority,
                    has_extensions,
                    end_of_group,
                );
                let extension_headers = if has_extensions {
                    draft14_extension_headers().boxed()
                } else {
                    Just(ExtensionHeaders::default()).boxed()
                };
                let object = prop_oneof![
                    vec(any::<u8>(), 1..64)
                        .prop_map(|payload| SubgroupObject::new_payload(Bytes::from(payload))),
                    varint().prop_map(SubgroupObject::new_status),
                ];
                let field = (varint(), extension_headers, object).prop_map(
                    |(object_id_delta, extension_headers, subgroup_object)| SubgroupObjectField {
                        object_id_delta,
                        extension_headers,
                        subgroup_object,
                    },
                );
                (Just(header), vec(field, 0..6))
            },
        )
}

/// The objects of a draft-14 FETCH response, each with its full location.
pub(crate) fn draft14_fetch() -> impl Strategy<Value = Vec<FetchObjectField>> {
    let fetch_object = prop_oneof![
        vec(any::<u8>(), 1..64).prop_map(|payload| FetchObject::Payload(Bytes::from(payload))),
        prop_oneof![
            Just(ObjectStatus::Normal),
            Just(ObjectStatus::DoesNotExist),
            Just(ObjectStatus::EndOfGroup),
            Just(ObjectStatus::EndOfTrack),
        ]
        .prop_map(FetchObject::Status),
    ];
    let field = (
        varint(),
        varint(),
        varint(),
        any::<u8>(),
        draft14_extension_headers(),
        fetch_object,
    )
        .prop_map(
            |(group_id, subgroup_id, object_id, priority, extension_headers, fetch_object)| {
                FetchObjectField::new(
                    group_id,
                    subgroup_id,
                    object_id,
                    priority,
                    extension_headers,
                    fetch_object,
                )
            },
        );
    vec(field, 0..6)
}

/// A draft-18 subgroup stream: its start and objects with ascending ids. Only
/// a Normal object carries a payload or properties (§11.2.1.1, §11.2.1.2).
pub(crate) fn draft18_subgroup() -> impl Strategy<Value = (SubgroupStart, Vec<Object>)> {
    (
        vi64(),
        prop_oneof![Just(0u64), vi64()],
        any::<u8>(),
        any::<bool>(),
    )
        .prop_flat_map(|(group_id, subgroup_id, publisher_priority, ends_group)| {
            let start = SubgroupStart {
                group_id,
                subgroup_id,
                publisher_priority,
                ends_group,
            };
            let body = prop_oneof![
                3 => (draft18_properties(), payload())
                    .prop_map(|(properties, payload)| (ObjectStatus::Normal, properties, payload)),
                1 => prop_oneof![Just(ObjectStatus::EndOfGroup), Just(ObjectStatus::EndOfTrack)]
                    .prop_map(|status| (status, ExtensionHeaders::default(), Bytes::new())),
            ];
            let objects = vec((gap(), body), 0..6).prop_map(move |bodies| {
                let mut object_id = None;
                bodies
                    .into_iter()
                    .map(|(gap, (status, extension_headers, payload))| {
                        let id = object_id.map_or(gap, |previous: u64| previous + 1 + gap);
                        object_id = Some(id);
                        Object {
                            location: Location {
                                group_id,
                                object_id: id,
                            },
                            forwarding: ForwardingPreference::Subgroup { subgroup_id },
                            publisher_priority,
                            status,
                            extension_headers,
                            payload,
                            ends_group: false,
                        }
                    })
                    .collect::<Vec<_>>()
            });
            (Just(start), objects)
        })
}

/// The Normal objects of a FETCH response (draft-18 FETCH objects carry no
/// status, §11.2.1.1): groups in `group_order`, objects in each group in
/// ascending order, each sent on any subgroup or as a datagram.
pub(crate) fn draft18_fetch() -> impl Strategy<Value = (GroupOrder, Vec<Object>)> {
    let forwarding = prop_oneof![
        Just(ForwardingPreference::Datagram),
        (0..4u64).prop_map(|subgroup_id| ForwardingPreference::Subgroup { subgroup_id }),
        vi64().prop_map(|subgroup_id| ForwardingPreference::Subgroup { subgroup_id }),
    ];
    let object = (
        gap(),
        forwarding,
        prop_oneof![Just(0u8), Just(128u8), any::<u8>()],
        draft18_properties(),
        payload(),
    );
    let group = (gap(), vec(object, 1..4));
    (
        prop_oneof![Just(GroupOrder::Ascending), Just(GroupOrder::Descending)],
        vec(group, 1..4),
    )
        .prop_map(|(group_order, groups)| {
            let mut group_id = None;
            let mut objects = Vec::new();
            for (group_gap, group) in groups {
                let id = group_id.map_or(group_gap, |previous: u64| previous + 1 + group_gap);
                group_id = Some(id);
                let mut object_id = None;
                let group_objects: Vec<Object> = group
                    .into_iter()
                    .map(
                        |(gap, forwarding, publisher_priority, extension_headers, payload)| {
                            let object = object_id.map_or(gap, |previous: u64| previous + 1 + gap);
                            object_id = Some(object);
                            Object {
                                location: Location {
                                    group_id: id,
                                    object_id: object,
                                },
                                forwarding,
                                publisher_priority,
                                status: ObjectStatus::Normal,
                                extension_headers,
                                payload,
                                ends_group: false,
                            }
                        },
                    )
                    .collect();
                objects.push(group_objects);
            }
            if group_order == GroupOrder::Descending {
                objects.reverse();
            }
            (group_order, objects.into_iter().flatten().collect())
        })
}

use bytes::Bytes;

use crate::modules::moqt::{
    control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::group_order::GroupOrder,
    },
    version::draft18::codec::get_key_value_pairs,
};

// Track Property types (draft-18 §12) the accessors read. Draft-14 carries
// the same values as parameters or fields and maps them to these types.
pub(crate) const OBJECT_DELIVERY_TIMEOUT: u64 = 0x02;
pub(crate) const MAX_CACHE_DURATION: u64 = 0x04;
pub(crate) const IMMUTABLE_PROPERTIES: u64 = 0x0B;
pub(crate) const DEFAULT_PUBLISHER_PRIORITY: u64 = 0x0E;
pub(crate) const DEFAULT_PUBLISHER_GROUP_ORDER: u64 = 0x22;

/// What PUBLISH, SUBSCRIBE_OK, TRACK_STATUS_OK and FETCH_OK say about the
/// track: parameters and fields in draft-14, Track Properties in draft-18
/// (§2.5).
///
/// The property list is the data; the accessors read it, in the mutable list
/// first and then inside Immutable Properties (§12.7). Received draft-18
/// properties keep the bytes they arrived in and go out in them again, as
/// §2.5 requires for a property a relay does not support; a setter replaces
/// only the entries of its type, and only those are encoded afresh. Draft-14
/// sends only the values it has parameters or fields for. Equality compares
/// the properties, not their encoding.
#[derive(Debug, Clone, Default)]
pub struct TrackProperties {
    /// In ascending type order, as Delta Types require.
    properties: Vec<TrackProperty>,
}

#[derive(Debug, Clone)]
pub(crate) struct TrackProperty {
    pub(crate) pair: KeyValuePair,
    /// Absent for a property set here rather than received.
    pub(crate) received: Option<ReceivedEncoding>,
}

/// The bytes a draft-18 Key-Value-Pair arrived in (§1.4.3).
#[derive(Debug, Clone)]
pub(crate) struct ReceivedEncoding {
    pub(crate) delta_type: u64,
    pub(crate) delta_type_bytes: Bytes,
    /// The value, with its length for an odd type.
    pub(crate) value_bytes: Bytes,
}

impl PartialEq for TrackProperties {
    fn eq(&self, other: &Self) -> bool {
        self.properties
            .iter()
            .map(|property| &property.pair)
            .eq(other.properties.iter().map(|property| &property.pair))
    }
}

impl Eq for TrackProperties {}

impl TrackProperties {
    /// Properties set here rather than received, in any order.
    pub fn from_pairs(mut pairs: Vec<KeyValuePair>) -> Self {
        // Stable, so properties of one type keep their order.
        pairs.sort_by_key(|pair| pair.key);
        Self {
            properties: pairs
                .into_iter()
                .map(|pair| TrackProperty {
                    pair,
                    received: None,
                })
                .collect(),
        }
    }

    /// Received properties, in the order they arrived, which ascends.
    pub(crate) fn received(properties: Vec<TrackProperty>) -> Self {
        Self { properties }
    }

    pub(crate) fn entries(&self) -> &[TrackProperty] {
        &self.properties
    }

    /// The mutable list, in ascending type order.
    pub fn iter(&self) -> impl Iterator<Item = &KeyValuePair> {
        self.properties.iter().map(|property| &property.pair)
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    /// MAX_CACHE_DURATION in milliseconds (draft-14 §9.2.1.3, draft-18
    /// §12.3).
    pub fn max_cache_duration(&self) -> Option<u64> {
        self.varint(MAX_CACHE_DURATION)
    }

    pub fn set_max_cache_duration(&mut self, milliseconds: Option<u64>) {
        self.set_varint(MAX_CACHE_DURATION, milliseconds);
    }

    /// OBJECT_DELIVERY_TIMEOUT in milliseconds (draft-14 §9.2.1.2 DELIVERY
    /// TIMEOUT, draft-18 §12.2).
    pub fn delivery_timeout(&self) -> Option<u64> {
        self.varint(OBJECT_DELIVERY_TIMEOUT)
    }

    pub fn set_delivery_timeout(&mut self, milliseconds: Option<u64>) {
        self.set_varint(OBJECT_DELIVERY_TIMEOUT, milliseconds);
    }

    /// DEFAULT PUBLISHER GROUP ORDER (draft-18 §12.5), or the Group Order
    /// of a draft-14 PUBLISH or SUBSCRIBE_OK. Absent means Ascending.
    pub fn group_order(&self) -> Option<GroupOrder> {
        match self.varint(DEFAULT_PUBLISHER_GROUP_ORDER)? {
            0x1 => Some(GroupOrder::Ascending),
            0x2 => Some(GroupOrder::Descending),
            _ => None,
        }
    }

    /// `Publisher` and `None` both remove it.
    pub fn set_group_order(&mut self, group_order: Option<GroupOrder>) {
        let value = group_order
            .filter(|order| *order != GroupOrder::Publisher)
            .map(|order| u64::from(u8::from(order)));
        self.set_varint(DEFAULT_PUBLISHER_GROUP_ORDER, value);
    }

    /// DEFAULT PUBLISHER PRIORITY (draft-18 §12.4).
    pub fn default_publisher_priority(&self) -> Option<u8> {
        self.varint(DEFAULT_PUBLISHER_PRIORITY)
            .and_then(|priority| u8::try_from(priority).ok())
    }

    /// The mutable list followed by the contents of Immutable Properties
    /// that parse, the order §12.7 searches a property in.
    pub(crate) fn searched(&self) -> Vec<KeyValuePair> {
        let mut searched: Vec<KeyValuePair> = self.iter().cloned().collect();
        for pair in self.iter() {
            if let KeyValuePair {
                key: IMMUTABLE_PROPERTIES,
                value: VariantType::Odd(contents),
            } = pair
                && let Ok(immutable) = get_key_value_pairs(&mut contents.clone())
            {
                searched.extend(immutable);
            }
        }
        searched
    }

    fn varint(&self, key: u64) -> Option<u64> {
        self.searched().into_iter().find_map(|pair| match pair {
            KeyValuePair {
                key: found,
                value: VariantType::Even(value),
            } if found == key => Some(value),
            _ => None,
        })
    }

    /// Replaces the entries of `key` in the mutable list with `value`; an
    /// entry that already holds just `value` keeps its received bytes.
    fn set_varint(&mut self, key: u64, value: Option<u64>) {
        let pair = value.map(|value| KeyValuePair {
            key,
            value: VariantType::Even(value),
        });
        let current: Vec<&KeyValuePair> = self.iter().filter(|pair| pair.key == key).collect();
        if current.len() == usize::from(pair.is_some()) && current.first().copied() == pair.as_ref()
        {
            return;
        }
        let at = self
            .properties
            .iter()
            .position(|property| property.pair.key >= key)
            .unwrap_or(self.properties.len());
        self.properties.retain(|property| property.pair.key != key);
        if let Some(pair) = pair {
            self.properties.insert(
                at,
                TrackProperty {
                    pair,
                    received: None,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(key: u64, value: u64) -> KeyValuePair {
        KeyValuePair {
            key,
            value: VariantType::Even(value),
        }
    }

    #[test]
    fn a_setter_replaces_only_the_entries_of_its_type() {
        // Arrange: two delivery timeouts and an application property
        let mut properties = TrackProperties::from_pairs(vec![
            varint(0x38, 1),
            varint(OBJECT_DELIVERY_TIMEOUT, 100),
            varint(OBJECT_DELIVERY_TIMEOUT, 150),
        ]);

        // Act
        properties.set_delivery_timeout(Some(200));
        properties.set_max_cache_duration(Some(50));

        // Assert: no earlier value is left to win over the set one
        assert_eq!(
            properties.iter().cloned().collect::<Vec<_>>(),
            vec![
                varint(OBJECT_DELIVERY_TIMEOUT, 200),
                varint(MAX_CACHE_DURATION, 50),
                varint(0x38, 1),
            ]
        );
        assert_eq!(properties.delivery_timeout(), Some(200));
    }

    #[test]
    fn setting_none_removes_the_property() {
        // Arrange
        let mut properties = TrackProperties::from_pairs(vec![
            varint(MAX_CACHE_DURATION, 50),
            varint(DEFAULT_PUBLISHER_GROUP_ORDER, 2),
        ]);

        // Act
        properties.set_max_cache_duration(None);
        properties.set_group_order(Some(GroupOrder::Publisher));

        // Assert: §12.5, an omitted group order means Ascending
        assert!(properties.is_empty());
        assert_eq!(properties.group_order(), None);
    }

    #[test]
    fn a_value_set_in_the_mutable_list_is_read_before_an_immutable_one() {
        // Arrange: MAX_CACHE_DURATION 50 inside Immutable Properties
        let mut properties = TrackProperties::from_pairs(vec![KeyValuePair {
            key: IMMUTABLE_PROPERTIES,
            value: VariantType::Odd(Bytes::from_static(&[0x04, 0x32])),
        }]);
        let immutable = properties.max_cache_duration();

        // Act
        properties.set_max_cache_duration(Some(70));

        // Assert: §12.7 searches the mutable list first
        assert_eq!(immutable, Some(50));
        assert_eq!(properties.max_cache_duration(), Some(70));
    }
}

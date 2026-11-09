use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, find_even},
        messages::parameters::{
            content_exists::ContentExists, group_order::GroupOrder,
            track_properties::TrackProperties,
        },
    },
};
use anyhow::Context;
use bytes::{Buf, BufMut, BytesMut};

#[derive(Debug, PartialEq, Clone)]
pub struct SubscribeOk {
    pub request_id: u64,
    pub track_alias: u64,
    pub expires: u64,
    /// The order the subscription is delivered in: the Group Order of a
    /// draft-14 SUBSCRIBE_OK. Draft-18 sends DEFAULT PUBLISHER GROUP ORDER
    /// in `track_properties` instead.
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    /// Holds the DELIVERY_TIMEOUT and MAX_CACHE_DURATION parameters of a
    /// draft-14 SUBSCRIBE_OK, and its Group Order as received.
    pub track_properties: TrackProperties,
}

impl SubscribeOk {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let track_alias = buf.try_get_varint().context("track alias")?;
        let expires = buf.try_get_varint().context("expires")?;
        let group_order_u8 = buf.try_get_u8().context("group order u8")?;

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = GroupOrder::try_from(group_order_u8).context("group order")?;

        let content_exists = ContentExists::decode(buf)?;

        let parameters = KeyValuePair::decode_list(buf)?;
        let mut track_properties = TrackProperties::default();
        track_properties.set_group_order(Some(group_order));
        track_properties.set_delivery_timeout(find_even(&parameters, 0x02));
        track_properties.set_max_cache_duration(find_even(&parameters, 0x04));

        Ok(SubscribeOk {
            request_id,
            track_alias,
            expires,
            group_order,
            content_exists,
            track_properties,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.track_alias);
        payload.put_varint(self.expires);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.content_exists.encode());
        let parameters: Vec<KeyValuePair> = [
            self.track_properties
                .delivery_timeout()
                .map(|value| KeyValuePair::even(0x02, value)),
            self.track_properties
                .max_cache_duration()
                .map(|value| KeyValuePair::even(0x04, value)),
        ]
        .into_iter()
        .flatten()
        .collect();
        payload.unsplit(KeyValuePair::encode_list(&parameters));
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::{
                content_exists::ContentExists, group_order::GroupOrder, location::Location,
                track_properties::TrackProperties,
            },
            subscribe_ok::SubscribeOk,
        };
        use bytes::BytesMut;

        #[test]
        fn packetize_content_not_exists() {
            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = ContentExists::False;

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                track_properties: Default::default(),
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                0, // Request ID (i)
                1, // Track alias (i)
                1, // Expires (i)
                1, // Group Order (8)
                0, // Content Exists (f)
                0, // Track Request Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn packetize_content_exists() {
            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = ContentExists::True {
                location: Location {
                    group_id: 10,
                    object_id: 20,
                },
            };

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                track_properties: Default::default(),
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                1,  // Content Exists (f)
                10, // Largest Group ID (i)
                20, // Largest Object ID (i)
                0,  // Track Request Parameters (..): Number of Parameters
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_content_not_exists() {
            let bytes_array = [
                0, // Request ID (i)
                1, // Track alias (i)
                1, // Expires (i)
                2, // Group Order (8)
                0, // Content Exists (f)
                0, // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_subscribe_ok = SubscribeOk::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = ContentExists::False;

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                // The Group Order is also the track's, for a draft-18 peer
                track_properties: {
                    let mut properties = TrackProperties::default();
                    properties.set_group_order(Some(group_order));
                    properties
                },
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }

        #[test]
        fn depacketize_content_exists() {
            let bytes_array = [
                0, // Request ID (i)
                2, // Track alias (i)
                1, // Expires (i)
                1, // Group Order (8)
                1, // Content Exists (f)
                0, // Largest Group ID (i)
                5, // Largest Object ID (i)
                0, // Track Request Parameters (..): Number of Parameters
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_subscribe_ok = SubscribeOk::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 2;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = ContentExists::True {
                location: Location {
                    group_id: 0,
                    object_id: 5,
                },
            };

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                // The Group Order is also the track's, for a draft-18 peer
                track_properties: {
                    let mut properties = TrackProperties::default();
                    properties.set_group_order(Some(group_order));
                    properties
                },
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }
    }
}

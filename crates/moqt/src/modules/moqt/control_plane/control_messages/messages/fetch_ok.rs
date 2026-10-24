use anyhow::Context;
use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, find_even},
        messages::parameters::{
            group_order::GroupOrder, location::Location, track_properties::TrackProperties,
        },
        util,
    },
};

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.17
// FETCH_OK Message {
//   Type (i) = 0x18,
//   Length (16),
//   Request ID (i),
//   Group Order (8),
//   End Of Track (8),
//   End Location (Location),
//   Number of Parameters (i),
//   Parameters (..) ...
// }

#[derive(Debug, Clone, PartialEq)]
pub struct FetchOk {
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub end_of_track: bool,
    pub end_location: Location,
    pub track_properties: TrackProperties,
}

impl FetchOk {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let group_order_u8 = buf.try_get_u8().context("group order u8")?;
        let group_order = GroupOrder::try_from(group_order_u8).context("group order")?;
        let end_of_track_u8 = buf.try_get_u8().context("end of track u8")?;
        let end_of_track = util::u8_to_bool(end_of_track_u8).context("end of track")?;
        let end_location = Location::decode(buf)?;
        let parameters = KeyValuePair::decode_list(buf)?;
        let track_properties = TrackProperties {
            max_cache_duration: find_even(&parameters, 0x04),
            other: Vec::new(),
        };
        Ok(FetchOk {
            request_id,
            group_order,
            end_of_track,
            end_location,
            track_properties,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.group_order as u8);
        payload.put_u8(self.end_of_track as u8);
        payload.unsplit(self.end_location.encode());
        let parameters: Vec<KeyValuePair> = self
            .track_properties
            .max_cache_duration
            .map(|value| KeyValuePair::even(0x04, value))
            .into_iter()
            .collect();
        payload.unsplit(KeyValuePair::encode_list(&parameters));
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::*;

        #[test]
        fn packetize() {
            let fetch_ok = FetchOk {
                request_id: 7,
                group_order: GroupOrder::Ascending,
                end_of_track: false,
                end_location: Location {
                    group_id: 10,
                    object_id: 5,
                },
                track_properties: Default::default(),
            };
            let result = fetch_ok.encode();
            let expected = [
                7,  // Request ID
                1,  // Group Order = Ascending
                0,  // End of Track = false
                10, // End Location: group_id
                5,  // End Location: object_id
                0,  // Number of Parameters
            ];
            assert_eq!(result.as_ref(), expected.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes: [u8; 6] = [7, 1, 0, 10, 5, 0];
            let mut cursor = std::io::Cursor::new(&bytes[..]);
            let result = FetchOk::decode(&mut cursor).unwrap();
            let expected = FetchOk {
                request_id: 7,
                group_order: GroupOrder::Ascending,
                end_of_track: false,
                end_location: Location {
                    group_id: 10,
                    object_id: 5,
                },
                track_properties: Default::default(),
            };
            assert_eq!(result, expected);
        }

        #[test]
        fn round_trip_end_of_track_true() {
            let expected = FetchOk {
                request_id: 3,
                group_order: GroupOrder::Descending,
                end_of_track: true,
                end_location: Location {
                    group_id: 100,
                    object_id: 200,
                },
                track_properties: TrackProperties {
                    max_cache_duration: Some(5000),
                    ..Default::default()
                },
            };
            let buf = expected.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let result = FetchOk::decode(&mut cursor).unwrap();
            assert_eq!(result, expected);
        }
    }
}

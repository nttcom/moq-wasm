use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{group_order::GroupOrder, location::Location},
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
    pub max_cache_duration: Option<u64>,
}

impl FetchOk {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let end_of_track_u8 = buf.try_get_u8().log_context("end of track u8").ok()?;
        let end_of_track = util::u8_to_bool(end_of_track_u8)
            .log_context("end of track")
            .ok()?;
        let end_location = Location::decode(buf)?;
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let kv = KeyValuePair::decode(buf)?;
            parameters.push(kv);
        }
        let max_cache_duration =
            parameters
                .iter()
                .find(|kv| kv.key == 0x04)
                .map(|kv| match kv.value {
                    VariantType::Odd(_) => unreachable!(),
                    VariantType::Even(value) => value,
                });
        Some(FetchOk {
            request_id,
            group_order,
            end_of_track,
            end_location,
            max_cache_duration,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.group_order as u8);
        payload.put_u8(self.end_of_track as u8);
        payload.unsplit(self.end_location.encode());
        let mut number_of_parameters = 0u64;
        let mut parameters_payload = BytesMut::new();
        if let Some(max_cache_duration) = self.max_cache_duration {
            let kv = KeyValuePair {
                key: 0x04,
                value: VariantType::Even(max_cache_duration),
            }
            .encode();
            parameters_payload.unsplit(kv);
            number_of_parameters += 1;
        }
        payload.put_varint(number_of_parameters);
        payload.unsplit(parameters_payload);
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
                max_cache_duration: None,
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
                max_cache_duration: None,
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
                max_cache_duration: Some(5000),
            };
            let buf = expected.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let result = FetchOk::decode(&mut cursor).unwrap();
            assert_eq!(result, expected);
        }
    }
}

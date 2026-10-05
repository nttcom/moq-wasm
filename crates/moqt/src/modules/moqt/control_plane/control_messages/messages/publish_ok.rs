use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::{filter_type::FilterType, group_order::GroupOrder},
        util,
    },
};

const DELIVERY_TIMEOUT: u64 = 0x02;

#[derive(Debug, PartialEq, Clone)]
pub struct PublishOk {
    pub request_id: u64,
    pub forward: bool,
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub filter_type: FilterType,
    pub delivery_timeout: Option<u64>,
}

impl PublishOk {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let forward_u8 = buf.try_get_u8().log_context("forward u8").ok()?;
        let forward = util::u8_to_bool(forward_u8).log_context("forward").ok()?;
        let subscriber_priority = buf.try_get_u8().log_context("subscriber priority").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let filter_type = FilterType::decode(buf)?;

        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut delivery_timeout = None;
        for _ in 0..number_of_parameters {
            if let KeyValuePair {
                key: DELIVERY_TIMEOUT,
                value: VariantType::Even(value),
            } = KeyValuePair::decode(buf)?
            {
                delivery_timeout = Some(value);
            }
        }

        Some(Self {
            request_id,
            forward,
            subscriber_priority,
            group_order,
            filter_type,
            delivery_timeout,
        })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.forward as u8);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.filter_type.encode());
        payload.put_varint(self.delivery_timeout.is_some() as u64);
        if let Some(delivery_timeout) = self.delivery_timeout {
            payload.unsplit(
                KeyValuePair {
                    key: DELIVERY_TIMEOUT,
                    value: VariantType::Even(delivery_timeout),
                }
                .encode(),
            );
        }

        tracing::trace!("Packetized Publish_OK message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::{filter_type::FilterType, group_order::GroupOrder, location::Location},
            publish_ok::PublishOk,
        };
        use bytes::Buf;

        fn round_trip(message: &PublishOk) -> PublishOk {
            let buf = message.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = PublishOk::decode(&mut cursor).unwrap();
            assert_eq!(cursor.remaining(), 0);
            decoded
        }

        #[test]
        fn packetize_and_depacketize_absolute_start() {
            // Arrange
            let publish_ok_message = PublishOk {
                request_id: 1,
                forward: true,
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                filter_type: FilterType::AbsoluteStart {
                    location: Location {
                        group_id: 10,
                        object_id: 5,
                    },
                },
                delivery_timeout: Some(1000),
            };

            // Act
            let depacketized_message = round_trip(&publish_ok_message);

            // Assert
            assert_eq!(depacketized_message, publish_ok_message);
        }

        #[test]
        fn packetize_and_depacketize_absolute_range() {
            // Arrange
            let publish_ok_message = PublishOk {
                request_id: 2,
                forward: false,
                subscriber_priority: 64,
                group_order: GroupOrder::Descending,
                filter_type: FilterType::AbsoluteRange {
                    location: Location {
                        group_id: 20,
                        object_id: 15,
                    },
                    end_group: 30,
                },
                delivery_timeout: Some(1000),
            };

            // Act
            let depacketized_message = round_trip(&publish_ok_message);

            // Assert
            assert_eq!(depacketized_message, publish_ok_message);
        }

        #[test]
        fn packetize_and_depacketize_without_delivery_timeout() {
            // Arrange
            let publish_ok_message = PublishOk {
                request_id: 3,
                forward: true,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending,
                filter_type: FilterType::NextGroupStart,
                delivery_timeout: None,
            };

            // Act
            let depacketized_message = round_trip(&publish_ok_message);

            // Assert
            assert_eq!(depacketized_message, publish_ok_message);
        }

        #[test]
        fn packetize_with_delivery_timeout_parameter() {
            // Arrange
            let publish_ok_message = PublishOk {
                request_id: 4,
                forward: true,
                subscriber_priority: 5,
                group_order: GroupOrder::Descending,
                filter_type: FilterType::NextGroupStart,
                delivery_timeout: Some(10),
            };

            // Act
            let buf = publish_ok_message.encode();

            // Assert
            let expected_bytes_array = [
                4,  // Request ID (i)
                1,  // Forward (8)
                5,  // Subscriber Priority (8)
                2,  // Group Order (8)
                1,  // Filter Type (i)
                1,  // Number of Parameters (i)
                2,  // Parameter Type (i): DELIVERY_TIMEOUT
                10, // Parameter Value (i)
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_skips_unknown_parameters() {
            // Arrange
            let bytes_array = [
                6,  // Request ID (i)
                1,  // Forward (8)
                5,  // Subscriber Priority (8)
                1,  // Group Order (8)
                1,  // Filter Type (i)
                2,  // Number of Parameters (i)
                4,  // Parameter Type (i): MAX_CACHE_DURATION
                7,  // Parameter Value (i)
                2,  // Parameter Type (i): DELIVERY_TIMEOUT
                30, // Parameter Value (i)
            ];
            let mut cursor = std::io::Cursor::new(&bytes_array[..]);

            // Act
            let depacketized_message = PublishOk::decode(&mut cursor).unwrap();

            // Assert
            assert_eq!(depacketized_message.delivery_timeout, Some(30));
            assert_eq!(cursor.remaining(), 0);
        }
    }
}

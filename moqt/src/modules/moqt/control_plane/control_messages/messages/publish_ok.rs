use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        messages::parameters::{filter_type::FilterType, group_order::GroupOrder},
        util,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct PublishOk {
    pub(crate) request_id: u64,
    pub(crate) forward: bool,
    pub(crate) subscriber_priority: u8,
    pub(crate) group_order: GroupOrder,
    pub(crate) filter_type: FilterType,
    pub(crate) delivery_timeout: Option<u64>,
}

impl PublishOk {
    pub(crate) fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let forward_u8 = buf.try_get_u8().log_context("forward u8").ok()?;
        let forward = util::u8_to_bool(forward_u8).log_context("forward").ok()?;
        let subscriber_priority = buf.try_get_u8().log_context("subscriber priority").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let filter_type = FilterType::decode(buf)?;
        let delivery_timeout =
            if let Ok(delivery_timeout) = buf.try_get_varint().log_context("delivery timeout") {
                Some(delivery_timeout)
            } else {
                None
            };

        Some(Self {
            request_id,
            forward,
            subscriber_priority,
            group_order,
            filter_type,
            delivery_timeout,
        })
    }

    pub(crate) fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.forward as u8);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.filter_type.encode());
        if let Some(delivery_timeout) = self.delivery_timeout {
            payload.put_varint(delivery_timeout);
        }

        tracing::trace!("Packetized Publish_OK message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;
        use crate::modules::moqt::control_plane::control_messages::messages::publish_ok::PublishOk;

        use crate::modules::moqt::control_plane::control_messages::messages::parameters::{
            filter_type::FilterType, group_order::GroupOrder,
        };
        #[test]
        fn packetize_and_depacketize_absolute_start() {
            let publish_ok_message = PublishOk {
                request_id: 1,
                forward: true,
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending, // Ascending
                filter_type: FilterType::AbsoluteStart {
                    location: Location {
                        group_id: 10,
                        object_id: 5,
                    },
                },
                delivery_timeout: Some(1000),
            };

            let buf = publish_ok_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_message = PublishOk::decode(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(publish_ok_message.forward, depacketized_message.forward);
            assert_eq!(
                publish_ok_message.subscriber_priority,
                depacketized_message.subscriber_priority
            );
            assert_eq!(
                publish_ok_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
        }

        #[test]
        fn packetize_and_depacketize_absolute_range() {
            let publish_ok_message = PublishOk {
                request_id: 2,
                forward: false,
                subscriber_priority: 64,
                group_order: GroupOrder::Descending, // Descending
                filter_type: FilterType::AbsoluteRange {
                    location: Location {
                        group_id: 20,
                        object_id: 15,
                    },
                    end_group: 30,
                },
                delivery_timeout: Some(1000),
            };

            let buf = publish_ok_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_message = PublishOk::decode(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(publish_ok_message.forward, depacketized_message.forward);
            assert_eq!(
                publish_ok_message.subscriber_priority,
                depacketized_message.subscriber_priority
            );
            assert_eq!(
                publish_ok_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            assert_eq!(
                publish_ok_message.delivery_timeout,
                depacketized_message.delivery_timeout
            );
        }

        #[test]
        fn packetize_and_depacketize_latest_group() {
            let publish_ok_message = PublishOk {
                request_id: 3,
                forward: true,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending, // Ascending
                filter_type: FilterType::LatestGroup,
                delivery_timeout: Some(1000),
            };

            let buf = publish_ok_message.encode();
            let mut buf = std::io::Cursor::new(&buf[..]);
            let depacketized_message = PublishOk::decode(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            assert_eq!(
                publish_ok_message.delivery_timeout,
                depacketized_message.delivery_timeout
            );
        }
    }
}

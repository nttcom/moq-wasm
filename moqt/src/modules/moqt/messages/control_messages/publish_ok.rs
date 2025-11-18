use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::messages::{
        control_messages::{
            enums::FilterType,
            group_order::GroupOrder,
            util::{self, add_payload_length, validate_payload_length},
            version_specific_parameters::VersionSpecificParameter,
        },
        moqt_payload::MOQTPayload,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct PublishOk {
    pub(crate) request_id: u64,
    pub(crate) forward: bool,
    pub(crate) subscriber_priority: u8,
    pub(crate) group_order: GroupOrder,
    pub(crate) filter_type: FilterType,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl PublishOk {
    pub(crate) fn decode(buf: &mut bytes::BytesMut) -> Option<Self> {
        if !validate_payload_length(buf) {
            return None;
        }
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
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf).ok()?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                parameters.push(version_specific_parameter);
            }
        }

        Some(Self {
            request_id,
            forward,
            subscriber_priority,
            group_order,
            filter_type,
            parameters,
        })
    }

    pub(crate) fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_u8(self.forward as u8);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.filter_type.encode());
        payload.put_varint(self.parameters.len() as u64);
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Publish_OK message.");
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {

        use crate::modules::moqt::messages::control_messages::{
            enums::FilterType,
            group_order::GroupOrder,
            location::Location,
            publish_ok::PublishOk,
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
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
                parameters: vec![VersionSpecificParameter::AuthorizationInfo(
                    AuthorizationInfo::new("token".to_string()),
                )],
            };

            let mut buf = publish_ok_message.encode();
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
                parameters: vec![],
            };

            let mut buf = publish_ok_message.encode();
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
            assert!(depacketized_message.parameters.is_empty());
        }

        #[test]
        fn packetize_and_depacketize_latest_group() {
            let publish_ok_message = PublishOk {
                request_id: 3,
                forward: true,
                subscriber_priority: 0,
                group_order: GroupOrder::Ascending, // Ascending
                filter_type: FilterType::LatestGroup,
                parameters: vec![],
            };

            let mut buf = publish_ok_message.encode();
            let depacketized_message = PublishOk::decode(&mut buf).unwrap();

            assert_eq!(
                publish_ok_message.request_id,
                depacketized_message.request_id
            );
            assert_eq!(
                publish_ok_message.filter_type,
                depacketized_message.filter_type
            );
            assert!(depacketized_message.parameters.is_empty());
        }
    }
}

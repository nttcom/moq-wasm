use bytes::{Buf, BufMut, BytesMut};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        messages::parameters::{
            content_exists::ContentExists, group_order::GroupOrder,
            version_specific_parameters::VersionSpecificParameter,
        },
        moqt_payload::MOQTPayload,
        util,
    },
};

pub(crate) struct Publish {
    pub(crate) request_id: u64,
    pub(crate) track_namespace_tuple: Vec<String>,
    pub(crate) track_name: String,
    pub(crate) track_alias: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: ContentExists,
    pub(crate) forward: bool,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl Publish {
    pub(crate) fn decode(buf: &mut bytes::BytesMut) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_namespace_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace tuple length")
            .ok()?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = buf.try_get_string().log_context("track namespace").ok()?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name = buf.try_get_string().log_context("track name").ok()?;
        let track_alias = buf.try_get_varint().log_context("track alias").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;
        let content_exists = ContentExists::decode(buf)?;

        let forward_u8 = buf.try_get_u8().log_context("forward u8").ok()?;
        let forward = util::u8_to_bool(forward_u8).log_context("forward").ok()?;
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
            track_namespace_tuple,
            track_name,
            track_alias,
            group_order,
            content_exists,
            forward,
            parameters,
        })
    }

    pub(crate) fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace_tuple.len();
        payload.put_varint(track_namespace_tuple_length as u64);
        self.track_namespace_tuple
            .iter()
            .for_each(|track_namespace| {
                payload.put_string(track_namespace);
            });
        payload.put_string(&self.track_name);
        payload.put_varint(self.track_alias);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.content_exists.encode());
        payload.put_u8(self.forward as u8);
        payload.put_varint(self.parameters.len() as u64);
        // Parameters
        for param in &self.parameters {
            param.packetize(&mut payload);
        }

        tracing::trace!("Packetized Publish message.");
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists;
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::group_order::GroupOrder;
        use crate::modules::moqt::control_plane::control_messages::messages::{
            parameters::location::Location, publish::Publish,
        };

        #[test]
        fn packetize_and_depacketize_with_location_and_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string(), "news".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending, // Ascending
                content_exists: ContentExists::True {
                    location: Location {
                        group_id: 10,
                        object_id: 5,
                    },
                },
                forward: true,
                parameters: vec![],
            };

            let mut buf = publish_message.encode();

            // depacketize
            let depacketized_message = Publish::decode(&mut buf).unwrap();

            assert_eq!(publish_message.request_id, depacketized_message.request_id);
            assert_eq!(
                publish_message.track_namespace_tuple,
                depacketized_message.track_namespace_tuple
            );
            assert_eq!(publish_message.track_name, depacketized_message.track_name);
            assert_eq!(
                publish_message.track_alias,
                depacketized_message.track_alias
            );
            assert_eq!(
                publish_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_message.content_exists,
                depacketized_message.content_exists
            );
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert_eq!(publish_message.parameters, depacketized_message.parameters);
        }

        #[test]
        fn packetize_and_depacketize_without_location_or_params() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "audio".to_string(),
                track_alias: 3,
                group_order: GroupOrder::Descending, // Descending
                content_exists: ContentExists::False,
                forward: false,
                parameters: vec![],
            };

            let mut buf = publish_message.encode();

            // depacketize
            let depacketized_message = Publish::decode(&mut buf).unwrap();

            assert_eq!(publish_message.request_id, depacketized_message.request_id);
            assert_eq!(
                publish_message.track_namespace_tuple,
                depacketized_message.track_namespace_tuple
            );
            assert_eq!(publish_message.track_name, depacketized_message.track_name);
            assert_eq!(
                publish_message.track_alias,
                depacketized_message.track_alias
            );
            assert_eq!(
                publish_message.group_order,
                depacketized_message.group_order
            );
            assert_eq!(
                publish_message.content_exists,
                depacketized_message.content_exists
            );
            assert_eq!(publish_message.forward, depacketized_message.forward);
            assert!(depacketized_message.parameters.is_empty());
        }

        #[test]
        fn packetize_check_bytes() {
            let publish_message = Publish {
                request_id: 1,
                track_namespace_tuple: vec!["moq".to_string()],
                track_name: "video".to_string(),
                track_alias: 2,
                group_order: GroupOrder::Ascending,
                content_exists: ContentExists::False,
                forward: true,
                parameters: vec![],
            };

            let buf = publish_message.encode();

            let expected_bytes = vec![
                1, 1, 3, b'm', b'o', b'q', 5, b'v', b'i', b'd', b'e', b'o', 2, 1, 0, 1, 0,
            ];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }
    }
}

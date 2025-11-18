use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::messages::{
        control_messages::{
            enums::ContentExists,
            group_order::GroupOrder,
            util::{add_payload_length, validate_payload_length},
            version_specific_parameters::VersionSpecificParameter,
        },
        moqt_payload::MOQTPayload,
    },
};
use bytes::{Buf, BufMut, BytesMut};

#[derive(Debug, PartialEq, Clone)]
pub struct SubscribeOk {
    pub(crate) request_id: u64,
    pub(crate) track_alias: u64,
    pub(crate) expires: u64,
    pub(crate) group_order: GroupOrder,
    pub(crate) content_exists: ContentExists,
    pub(crate) subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl SubscribeOk {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        if !validate_payload_length(buf) {
            return None;
        }
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let track_alias = buf.try_get_varint().log_context("track alias").ok()?;
        let expires = buf.try_get_varint().log_context("expires").ok()?;
        let group_order_u8 = buf.try_get_u8().log_context("group order u8").ok()?;

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = GroupOrder::try_from(group_order_u8)
            .log_context("group order")
            .ok()?;

        let content_exists = ContentExists::decode(buf)?;

        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut subscribe_parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf).ok()?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                subscribe_parameters.push(version_specific_parameter);
            }
        }

        Some(SubscribeOk {
            request_id,
            track_alias,
            expires,
            group_order,
            content_exists,
            subscribe_parameters,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.track_alias);
        payload.put_varint(self.expires);
        payload.put_u8(self.group_order as u8);
        payload.unsplit(self.content_exists.encode());
        payload.put_varint(self.subscribe_parameters.len() as u64);
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(&mut payload);
        }
        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::messages::control_messages::{
            enums::ContentExists,
            location::Location,
            subscribe_ok::{GroupOrder, SubscribeOk},
            version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
        };
        use bytes::BytesMut;

        #[test]
        fn packetize_content_not_exists() {
            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Ascending;
            let content_exists = ContentExists::False;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                subscribe_parameters,
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                12, // Message Length(i)
                0,  // Request ID (i)
                1,  // Track alias (i)
                1,  // Expires (i)
                1,  // Group Order (8)
                0,  // Content Exists (f)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
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
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                subscribe_parameters,
            };
            let buf = subscribe_ok.encode();

            let expected_bytes_array = [
                14, // Message Length(i)
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                1,  // Content Exists (f)
                10, // Largest Group ID (i)
                20, // Largest Object ID (i)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize_content_not_exists() {
            let bytes_array = [
                12, // Message Length(i)
                0,  // Request ID (i)
                1,  // Track alias (i)
                1,  // Expires (i)
                2,  // Group Order (8)
                0,  // Content Exists (f)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe_ok = SubscribeOk::decode(&mut buf).unwrap();

            let request_id = 0;
            let track_alias = 1;
            let expires = 1;
            let group_order = GroupOrder::Descending;
            let content_exists = ContentExists::False;
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }

        #[test]
        fn depacketize_content_exists() {
            let bytes_array = [
                14, // Message Length(i)
                0,  // Request ID (i)
                2,  // Track alias (i)
                1,  // Expires (i)
                1,  // Group Order (8)
                1,  // Content Exists (f)
                0,  // Largest Group ID (i)
                5,  // Largest Object ID (i)
                1,  // Track Request Parameters (..): Number of Parameters
                2,  // Parameter Type (i): AuthorizationInfo
                4,  // Parameter Length (i)
                116, 101, 115, 116, // Parameter Value (..): test
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
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
            let version_specific_parameter = VersionSpecificParameter::AuthorizationInfo(
                AuthorizationInfo::new("test".to_string()),
            );
            let subscribe_parameters = vec![version_specific_parameter];

            let expected_subscribe_ok = SubscribeOk {
                request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                subscribe_parameters,
            };

            assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
        }
    }
}

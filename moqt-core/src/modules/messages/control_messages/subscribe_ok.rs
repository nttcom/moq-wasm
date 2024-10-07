use crate::messages::control_messages::version_specific_parameters::VersionSpecificParameter;
use crate::messages::moqt_payload::MOQTPayload;
use crate::modules::variable_integer::{read_variable_integer_from_buffer, write_variable_integer};
use crate::variable_bytes::read_fixed_length_bytes_from_buffer;
use anyhow::bail;
use anyhow::Context;
use serde::Serialize;
use std::any::Any;

use super::subscribe::GroupOrder;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeOk {
    subscribe_id: u64,
    expires: u64,
    group_order: GroupOrder,
    content_exists: bool,
    largest_group_id: Option<u64>,
    largest_object_id: Option<u64>,
    number_of_parameters: u64,
    subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl SubscribeOk {
    pub fn new(
        subscribe_id: u64,
        expires: u64,
        group_order: GroupOrder,
        content_exists: bool,
        largest_group_id: Option<u64>,
        largest_object_id: Option<u64>,
        subscribe_parameters: Vec<VersionSpecificParameter>,
    ) -> Self {
        let number_of_parameters = subscribe_parameters.len() as u64;
        SubscribeOk {
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            number_of_parameters,
            subscribe_parameters,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn replace_subscribe_id(&mut self, subscribe_id: u64) {
        self.subscribe_id = subscribe_id;
    }
}

impl MOQTPayload for SubscribeOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe_id")?;
        let expires = read_variable_integer_from_buffer(buf).context("expires")?;
        let group_order_u8 = read_fixed_length_bytes_from_buffer(buf, 1)?[0];

        // Values larger than 0x2 are a protocol error.
        let group_order = match GroupOrder::try_from(group_order_u8).context("group order") {
            Ok(group_order) => group_order,
            Err(err) => {
                // TODO: return Termination Error Code
                bail!(err);
            }
        };

        let content_exists =
            match read_fixed_length_bytes_from_buffer(buf, 1).context("content_exists")?[0] {
                0 => false,
                1 => true,
                _ => {
                    // TODO: return Termination Error Code
                    bail!("Invalid content_exists value: Protocol Violation");
                }
            };

        let (largest_group_id, largest_object_id) = if content_exists {
            let largest_group_id =
                read_variable_integer_from_buffer(buf).context("largest_group_id")?;
            let largest_object_id =
                read_variable_integer_from_buffer(buf).context("largest_object_id")?;
            (Some(largest_group_id), Some(largest_object_id))
        } else {
            (None, None)
        };

        let number_of_parameters =
            read_variable_integer_from_buffer(buf).context("number of parameters")?;
        let mut subscribe_parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                subscribe_parameters.push(version_specific_parameter);
            }
        }

        tracing::trace!("Depacketized Subscribe OK message.");

        Ok(SubscribeOk {
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            number_of_parameters,
            subscribe_parameters,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace Number of elements
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.expires));
        buf.extend(u8::from(self.group_order).to_be_bytes());
        buf.extend(u8::from(self.content_exists).to_be_bytes());
        if self.content_exists {
            buf.extend(write_variable_integer(self.largest_group_id.unwrap()));
            buf.extend(write_variable_integer(self.largest_object_id.unwrap()));
        }
        buf.extend(write_variable_integer(self.number_of_parameters));
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(buf);
        }

        tracing::trace!("Packetized Subscribe OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeOk
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::control_messages::{
        subscribe_ok::GroupOrder,
        version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
    };
    use crate::{
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::subscribe_ok::SubscribeOk,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe_ok_content_not_exists() {
        let subscribe_id = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            1, // Expires (i)
            1, // Group Order (8)
            0, // Content Exists (f)
            1, // Track Request Parameters (..): Number of Parameters
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn packetize_subscribe_ok_content_exists() {
        let subscribe_id = 0;
        let expires = 1;
        let group_order = GroupOrder::Descending;
        let content_exists = true;
        let largest_group_id = Some(10);
        let largest_object_id = Some(20);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe_ok = SubscribeOk::new(
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );
        let mut buf = bytes::BytesMut::new();
        subscribe_ok.packetize(&mut buf);

        let expected_bytes_array = [
            0,  // Subscribe ID (i)
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
    fn depacketize_subscribe_ok_content_not_exists() {
        let bytes_array = [
            0, // Subscribe ID (i)
            1, // Expires (i)
            2, // Group Order (8)
            0, // Content Exists (f)
            1, // Track Request Parameters (..): Number of Parameters
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let expires = 1;
        let group_order = GroupOrder::Descending;
        let content_exists = false;
        let largest_group_id = None;
        let largest_object_id = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let expected_subscribe_ok = SubscribeOk::new(
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );

        assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
    }

    #[test]
    fn depacketize_subscribe_ok_content_exists() {
        let bytes_array = [
            0, // Subscribe ID (i)
            1, // Expires (i)
            1, // Group Order (8)
            1, // Content Exists (f)
            0, // Largest Group ID (i)
            5, // Largest Object ID (i)
            1, // Track Request Parameters (..): Number of Parameters
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let expires = 1;
        let group_order = GroupOrder::Ascending;
        let content_exists = true;
        let largest_group_id = Some(0);
        let largest_object_id = Some(5);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let expected_subscribe_ok = SubscribeOk::new(
            subscribe_id,
            expires,
            group_order,
            content_exists,
            largest_group_id,
            largest_object_id,
            subscribe_parameters,
        );

        assert_eq!(depacketized_subscribe_ok, expected_subscribe_ok);
    }
}

#[cfg(test)]
mod failure {
    use crate::{
        messages::moqt_payload::MOQTPayload,
        modules::messages::control_messages::subscribe_ok::SubscribeOk,
    };
    use bytes::BytesMut;

    #[test]
    fn depacketize_subscribe_ok_invalid_group_order() {
        let bytes_array = [
            0,  // Subscribe ID (i)
            1,  // Expires (i)
            20, // Group Order (8)
            0,  // Content Exists (f)
            1,  // Track Request Parameters (..): Number of Parameters
            2,  // Parameter Type (i): AuthorizationInfo
            4,  // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf);

        assert!(depacketized_subscribe_ok.is_err());
    }

    #[test]
    fn depacketize_subscribe_ok_invalid_content_exist() {
        let bytes_array = [
            0, // Subscribe ID (i)
            1, // Expires (i)
            1, // Group Order (8)
            5, // Content Exists (f)
            0, // Largest Group ID (i)
            5, // Largest Object ID (i)
            1, // Track Request Parameters (..): Number of Parameters
            2, // Parameter Type (i): AuthorizationInfo
            4, // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe_ok = SubscribeOk::depacketize(&mut buf);

        assert!(depacketized_subscribe_ok.is_err());
    }
}

use super::version_specific_parameters::VersionSpecificParameter;
use crate::messages::moqt_payload::MOQTPayload;
use crate::{
    modules::{
        variable_bytes::{read_fixed_length_bytes_from_buffer, read_variable_bytes_from_buffer},
        variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
    },
    variable_bytes::write_variable_bytes,
};
use anyhow::{bail, Context};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
use std::any::Any;
use tracing;

#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum GroupOrder {
    Ascending = 0x1,
    Descending = 0x2,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum FilterType {
    LatestGroup = 0x1,
    LatestObject = 0x2,
    AbsoluteStart = 0x3,
    AbsoluteRange = 0x4,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Subscribe {
    subscribe_id: u64,
    track_alias: u64,
    track_namespace: Vec<String>,
    track_name: String,
    subscriber_priority: u8,
    group_order: GroupOrder,
    filter_type: FilterType,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
    end_object: Option<u64>,
    number_of_parameters: u64,
    subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl Subscribe {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        subscribe_id: u64,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
        subscribe_parameters: Vec<VersionSpecificParameter>,
    ) -> anyhow::Result<Subscribe> {
        // StartGroup and StartObject: Only present for "AbsoluteStart" and "AbsoluteRange"
        if !matches!(
            filter_type,
            FilterType::AbsoluteStart | FilterType::AbsoluteRange
        ) {
            if start_group.is_some() {
                // TODO: return Termination Error Code
                bail!(
                    "start_group must be None unless filter_type is AbsoluteStart or AbsoluteRange"
                );
            } else if start_object.is_some() {
                // TODO: return Termination Error Code
                bail!("start_object must be None unless filter_type is AbsoluteStart or AbsoluteRange");
            }
        }

        // EndGroup and EndObject: Only present for "AbsoluteRange"
        if !matches!(filter_type, FilterType::AbsoluteRange) {
            if end_group.is_some() {
                // TODO: return Termination Error Code
                bail!("end_group must be None unless filter_type is AbsoluteRange");
            } else if end_object.is_some() {
                // TODO: return Termination Error Code
                bail!("end_object must be None unless filter_type is AbsoluteRange");
            }
        }

        // TODO: Check the way if Start/End Group/Object is not present for AbsoluteStart/AbsoluteRange

        let number_of_parameters = subscribe_parameters.len() as u64;
        Ok(Subscribe {
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            number_of_parameters,
            subscribe_parameters,
        })
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    pub fn track_namespace(&self) -> &Vec<String> {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }

    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    pub fn group_order(&self) -> GroupOrder {
        self.group_order
    }

    pub fn filter_type(&self) -> FilterType {
        self.filter_type
    }

    pub fn start_group(&self) -> Option<u64> {
        self.start_group
    }

    pub fn start_object(&self) -> Option<u64> {
        self.start_object
    }

    pub fn end_group(&self) -> Option<u64> {
        self.end_group
    }

    pub fn end_object(&self) -> Option<u64> {
        self.end_object
    }

    pub fn set_subscribe_id(&mut self, subscribe_id: u64) {
        self.subscribe_id = subscribe_id;
    }

    pub fn set_track_alias(&mut self, track_alias: u64) {
        self.track_alias = track_alias;
    }
}

impl MOQTPayload for Subscribe {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;
        let track_alias = read_variable_integer_from_buffer(buf).context("track alias")?;
        let track_namespace_tuple_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let mut track_namespace_tuple: Vec<String> = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)
                .context("track namespace")?;
            track_namespace_tuple.push(track_namespace);
        }
        let track_name =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track name")?;
        let subscriber_priority =
            read_fixed_length_bytes_from_buffer(buf, 1).context("subscriber priority")?[0];
        let group_order_u8 = read_fixed_length_bytes_from_buffer(buf, 1)?[0];

        // Values larger than 0x2 are a Protocol Violation.
        let group_order = match GroupOrder::try_from(group_order_u8).context("group order") {
            Ok(group_order) => group_order,
            Err(err) => {
                // TODO: return Termination Error Code
                bail!(err);
            }
        };

        let filter_type_u64 = read_variable_integer_from_buffer(buf)?;

        // A filter type other than the above MUST be treated as error.
        let filter_type = match FilterType::try_from(filter_type_u64 as u8).context("filter type") {
            Ok(filter_type) => filter_type,
            Err(err) => {
                // TODO: return Termination Error Code
                bail!(err);
            }
        };
        let (start_group, start_object) = match filter_type {
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => (
                Some(read_variable_integer_from_buffer(buf).context("start group")?),
                Some(read_variable_integer_from_buffer(buf).context("start object")?),
            ),
            _ => (None, None),
        };

        let (end_group, end_object) = match filter_type {
            FilterType::AbsoluteRange => (
                Some(read_variable_integer_from_buffer(buf).context("end group")?),
                Some(read_variable_integer_from_buffer(buf).context("end object")?),
            ),
            _ => (None, None),
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

        tracing::trace!("Depacketized Subscribe message.");

        Ok(Subscribe {
            subscribe_id,
            track_alias,
            track_namespace: track_namespace_tuple,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            number_of_parameters,
            subscribe_parameters,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.track_alias));
        // Track Namespace Number of elements
        let track_namespace_tuple_length = self.track_namespace.len();
        buf.extend(write_variable_integer(track_namespace_tuple_length as u64));
        for track_namespace in &self.track_namespace {
            // Track Namespace
            buf.extend(write_variable_bytes(&track_namespace.as_bytes().to_vec()));
        }
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(self.subscriber_priority.to_be_bytes());
        buf.extend(u8::from(self.group_order).to_be_bytes());
        buf.extend(write_variable_integer(u8::from(self.filter_type) as u64));
        match self.filter_type {
            FilterType::AbsoluteStart => {
                buf.extend(write_variable_integer(self.start_group.unwrap()));
                buf.extend(write_variable_integer(self.start_object.unwrap()));
            }
            FilterType::AbsoluteRange => {
                buf.extend(write_variable_integer(self.start_group.unwrap()));
                buf.extend(write_variable_integer(self.start_object.unwrap()));
                buf.extend(write_variable_integer(self.end_group.unwrap()));
                buf.extend(write_variable_integer(self.end_object.unwrap()));
            }
            _ => {}
        }
        buf.extend(write_variable_integer(
            self.subscribe_parameters.len() as u64
        ));
        for version_specific_parameter in &self.subscribe_parameters {
            version_specific_parameter.packetize(buf);
        }

        tracing::trace!("Packetized Subscribe OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeRequest
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::control_messages::subscribe::{FilterType, GroupOrder, Subscribe};
    use crate::messages::control_messages::version_specific_parameters::AuthorizationInfo;
    use crate::messages::control_messages::version_specific_parameters::VersionSpecificParameter;
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe_latest_group() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        let mut buf = BytesMut::new();
        subscribe.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            1,   // Filter Type (i): LatestGroup
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn packetize_subscribe_absolute_start() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Descending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        let mut buf = BytesMut::new();
        subscribe.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            2,   // Group Order (8): Descending
            3,   // Filter Type (i): AbsoluteStart
            0,   // Start Group (i)
            0,   // Start Object (i)
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn packetize_subscribe_absolute_range() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteRange;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = Some(10);
        let end_object = Some(100);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        let mut buf = BytesMut::new();
        subscribe.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            4,   // Filter Type (i): AbsoluteRange
            0,   // Start Group (i)
            0,   // Start Object (i)
            10,  // End Group (i)
            64, 100, // End Object (i)
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_subscribe_latest_group() {
        let bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            1,   // Filter Type (i): LatestGroup
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length (i)
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];
        let expected_subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        assert_eq!(depacketized_subscribe, expected_subscribe);
    }

    #[test]
    fn depacketize_subscribe_absolute_start() {
        let bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            3,   // Filter Type (i): AbsoluteStart
            0,   // Start Group (i)
            0,   // Start Object (i)
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];
        let expected_subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        assert_eq!(depacketized_subscribe, expected_subscribe);
    }

    #[test]
    fn depacketize_subscribe_absolute_range() {
        let bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            4,   // Filter Type (i): AbsoluteRange
            0,   // Start Group (i)
            0,   // Start Object (i)
            10,  // End Group (i)
            64, 100, // End Object (i)
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteRange;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = Some(10);
        let end_object = Some(100);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];
        let expected_subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        )
        .unwrap();

        assert_eq!(depacketized_subscribe, expected_subscribe);
    }
}

#[cfg(test)]
mod failure {
    use crate::messages::control_messages::subscribe::{FilterType, GroupOrder, Subscribe};
    use crate::messages::control_messages::version_specific_parameters::AuthorizationInfo;
    use crate::messages::control_messages::version_specific_parameters::VersionSpecificParameter;
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe_latest_group_with_start_parameter() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;
        let end_object = None;
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        );

        assert!(subscribe.is_err());
    }

    #[test]
    fn packetize_subscribe_latest_group_with_end_parameter() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::LatestGroup;
        let start_group = None;
        let start_object = None;
        let end_group = Some(1);
        let end_object = Some(1);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        );

        assert!(subscribe.is_err());
    }

    #[test]
    fn packetize_subscribe_absolute_start_with_end_parameter() {
        let subscribe_id = 0;
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = Some(1);
        let end_object = Some(1);
        let version_specific_parameter =
            VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new("test".to_string()));
        let subscribe_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            subscribe_id,
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscribe_parameters,
        );

        assert!(subscribe.is_err());
    }

    #[test]
    fn depacketize_unknown_filter_type() {
        let bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            1,   // Group Order (8): Assending
            5,   // Filter Type (i): Unknown
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf);

        println!("{:?}", depacketized_subscribe);

        assert!(depacketized_subscribe.is_err());
    }

    #[test]
    fn depacketize_unknown_group_order() {
        let bytes_array = [
            0, // Subscribe ID (i)
            0, // Track Alias (i)
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            4,   // Track Namespace(b): Length
            116, 101, 115, 116, // Track Namespace(b): Value("test")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // Subscriber Priority (8)
            3,   // Group Order (8): Unknown
            1,   // Filter Type (i): LatestGroup
            1,   // Track Request Parameters (..): Number of Parameters
            2,   // Parameter Type (i): AuthorizationInfo
            4,   // Parameter Length
            116, 101, 115, 116, // Parameter Value (..): test
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf);

        println!("{:?}", depacketized_subscribe);

        assert!(depacketized_subscribe.is_err());
    }
}

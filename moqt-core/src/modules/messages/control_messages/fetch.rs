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

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Fetch {
    subscribe_id: u64,
    track_namespace: Vec<String>,
    track_name: String,
    subscriber_priority: u8,
    group_order: GroupOrder,
    start_group: u64,
    start_object: u64,
    end_group: u64,
    end_object: u64,
    number_of_parameters: u64,
    subscribe_parameters: Vec<VersionSpecificParameter>,
}

impl Fetch {
    pub fn new(
        subscribe_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        end_object: u64,
        subscribe_parameters: Vec<VersionSpecificParameter>,
    ) -> anyhow::Result<Fetch> {
        let number_of_parameters = subscribe_parameters.len() as u64;
        Ok(Fetch {
            subscribe_id,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
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

    pub fn start_group(&self) -> u64 {
        self.start_group
    }

    pub fn start_object(&self) -> u64 {
        self.start_object
    }

    pub fn end_group(&self) -> u64 {
        self.end_group
    }

    pub fn end_object(&self) -> u64 {
        self.end_object
    }

    pub fn subscribe_parameters(&self) -> &Vec<VersionSpecificParameter> {
        &self.subscribe_parameters
    }
}

impl MOQTPayload for Fetch {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;
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

        tracing::trace!("Depacketized Fetch message.");

        Ok(Fetch {
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

        tracing::trace!("Packetized Fetch OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to FetchRequest
    fn as_any(&self) -> &dyn Any {
        self
    }
}

use anyhow::{bail, Context};
use serde::Serialize;
use std::any::Any;
use tracing;

use crate::{
    modules::{
        variable_bytes::read_variable_bytes_from_buffer,
        variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
    },
    variable_bytes::write_variable_bytes,
};

use super::{moqt_payload::MOQTPayload, version_specific_parameters::VersionSpecificParameter};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Subscribe {
    track_namespace: String,
    track_name: String,
    start_group: Location,
    start_object: Location,
    end_group: Location,
    end_object: Location,
    track_request_parameters: Vec<VersionSpecificParameter>,
}

impl Subscribe {
    pub fn new(
        track_namespace: String,
        track_name: String,
        start_group: Location,
        start_object: Location,
        end_group: Location,
        end_object: Location,
        track_request_parameters: Vec<VersionSpecificParameter>,
    ) -> Subscribe {
        Subscribe {
            track_namespace,
            track_name,
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }
}

impl MOQTPayload for Subscribe {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let track_name = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let start_group = Location::depacketize(buf)?;
        let start_object = Location::depacketize(buf)?;
        let end_group = Location::depacketize(buf)?;
        let end_object = Location::depacketize(buf)?;

        // NOTE:
        //   number_of_parameters is not defined in draft-01, but defined in 03.
        //   For interoperability testing with meta moq, it is implemented in accordance with draft-03.
        let number_of_parameters = read_variable_integer_from_buffer(buf)?;
        let mut track_request_parameters = Vec::new();
        for _ in 0..number_of_parameters {
            let version_specific_parameter = VersionSpecificParameter::depacketize(buf)?;
            if let VersionSpecificParameter::Unknown(code) = version_specific_parameter {
                tracing::warn!("unknown track request parameter {}", code);
            } else {
                // NOTE:
                //   According to "6.1.1. Version Specific Parameters", the parameters used
                //   in the SUBSCRIBE message are Version Specific Parameters. On the other hand,
                //   according to "6.4.2. SUBSCRIBE REQUEST Format", it is the Track Request Parameters
                //   that are included in the SUBSCRIBE REQUEST Message, and refers to 6.1.1 for details.
                //   Therefore, version_specific_parameter is pushed to track_request_parameters.
                //     (https://datatracker.ietf.org/doc/html/draft-ietf-moq-transport-01)
                track_request_parameters.push(version_specific_parameter);
            }
        }

        tracing::trace!("Depacketized Subscribe message.");

        Ok(Subscribe {
            track_namespace,
            track_name,
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        self.start_group.packetize(buf);
        self.start_object.packetize(buf);
        self.end_group.packetize(buf);
        self.end_object.packetize(buf);
        // NOTE:
        //   number_of_parameters is not defined in draft-01, but defined in 03.
        //   For interoperability testing with meta moq, it is implemented in accordance with draft-03.
        buf.extend(write_variable_integer(
            self.track_request_parameters.len() as u64
        ));
        for version_specific_parameter in &self.track_request_parameters {
            version_specific_parameter.packetize(buf);
        }

        tracing::trace!("Packetized Subscribe OK message.");
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeRequest
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum Location {
    None,                  // 0x00
    Absolute(u64),         // 0x01
    RelativePrevious(u64), // 0x02
    RelativeNext(u64),     // 0x03
}

impl MOQTPayload for Location {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let location = read_variable_integer_from_buffer(buf).context("location")?;

        match location {
            0x00 => Ok(Location::None),
            0x01 => Ok(Location::Absolute(read_variable_integer_from_buffer(buf)?)),
            0x02 => Ok(Location::RelativePrevious(
                read_variable_integer_from_buffer(buf)?,
            )),
            0x03 => Ok(Location::RelativeNext(read_variable_integer_from_buffer(
                buf,
            )?)),
            _ => bail!("invalid location"),
        }
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        match self {
            Location::None => {
                buf.extend(write_variable_integer(0x00));
            }
            Location::Absolute(value) => {
                buf.extend(write_variable_integer(0x01));
                buf.extend(write_variable_integer(*value));
            }
            Location::RelativePrevious(value) => {
                buf.extend(write_variable_integer(0x02));
                buf.extend(write_variable_integer(*value));
            }
            Location::RelativeNext(value) => {
                buf.extend(write_variable_integer(0x03));
                buf.extend(write_variable_integer(*value));
            }
        }
    }
    /// Method to enable downcasting from MOQTPayload to Location
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::subscribe::Subscribe;
    use crate::messages::version_specific_parameters::GroupSequence;
    use crate::messages::{
        subscribe::Location, version_specific_parameters::VersionSpecificParameter,
    };
    use bytes::BytesMut;

    #[test]
    fn packetize_subscribe() {
        let track_namespace = "track_namespace".to_string();
        let track_name = "track_name".to_string();
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];

        let subscribe = Subscribe::new(
            track_namespace,
            track_name,
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        let mut buf = BytesMut::new();
        subscribe.packetize(&mut buf);

        let expected_bytes_array = [
            15, // Track Namespace (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace (b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // StartGroup (Location): Location(None)
            0,   // StartObject (Location): Location(None)
            0,   // EndGroup (Location): Location(None)
            0,   // EndObject (Location): Location(None)
            1,   // Track Request Parameters (..): Number of Parameters
            0,   // Parameter Type (i)
            1,   // Parameter Length (i)
            0,   // Parameter Value (..): GroupSequence
        ];
        assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
    }

    #[test]
    fn depacketize_subscribe() {
        let bytes_array = [
            15, // Track Namespace (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace (b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // StartGroup (Location): Location(None)
            0,   // StartObject (Location): Location(None)
            0,   // EndGroup (Location): Location(None)
            0,   // EndObject (Location): Location(None)
            1,   // Track Request Parameters (..): Number of Parameters
            0,   // Parameter Type (i)
            1,   // Parameter Length (i)
            0,   // Parameter Value (..): GroupSequence
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_subscribe = Subscribe::depacketize(&mut buf).unwrap();

        let track_namespace = "track_namespace".to_string();
        let track_name = "track_name".to_string();
        let start_group = Location::None;
        let start_object = Location::None;
        let end_group = Location::None;
        let end_object = Location::None;
        let version_specific_parameter =
            VersionSpecificParameter::GroupSequence(GroupSequence::new(0));
        let track_request_parameters = vec![version_specific_parameter];
        let expected_subscribe = Subscribe::new(
            track_namespace,
            track_name,
            start_group,
            start_object,
            end_group,
            end_object,
            track_request_parameters,
        );

        assert_eq!(depacketized_subscribe, expected_subscribe);
    }
}

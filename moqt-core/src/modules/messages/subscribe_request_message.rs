use anyhow::bail;
use tracing::info;

use crate::modules::{
    variable_bytes::read_variable_bytes_from_buffer,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::{moqt_payload::MOQTPayload, version_specific_parameters::TrackRequestParameter};

pub(crate) struct SubscribeRequestMessage {
    track_namespace: String,
    track_name: String,
    start_group: Location,
    start_object: Location,
    end_group: Location,
    end_object: Location,
    track_request_parameters: Vec<TrackRequestParameter>,
}

impl SubscribeRequestMessage {
    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }
}

impl MOQTPayload for SubscribeRequestMessage {
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

        let mut track_request_parameters = Vec::new();
        while !buf.is_empty() {
            let track_request_parameter = TrackRequestParameter::depacketize(buf)?;
            if let TrackRequestParameter::Unknown(code) = track_request_parameter {
                tracing::info!("unknown track request parameter {}", code);
            } else {
                track_request_parameters.push(track_request_parameter);
            }
        }

        Ok(SubscribeRequestMessage {
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
        todo!()
    }
}

#[derive(Debug)]
pub(crate) enum Location {
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
        let location = read_variable_integer_from_buffer(buf)?;

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
}

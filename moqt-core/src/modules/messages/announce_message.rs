use std::io::Cursor;

use anyhow::{Context, Result};

use crate::{
    modules::{
        messages::parameter::Parameter, variable_integer::read_variable_integer_from_buffer,
    },
    variable_bytes::{read_variable_bytes_with_length_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};

use super::moqt_payload::MOQTPayload;

pub struct AnnounceMessage {
    pub(crate) track_namespace: String,
    pub(crate) number_of_parameters: u8,
    pub(crate) parameters: Vec<Parameter>,
}

impl AnnounceMessage {
    pub fn new(
        track_namespace: String,
        number_of_parameters: u8,
        parameters: Vec<Parameter>,
    ) -> Self {
        AnnounceMessage {
            track_namespace,
            number_of_parameters,
            parameters,
        }
    }
    pub(crate) fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

impl MOQTPayload for AnnounceMessage {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let read_cur = Cursor::new(&buf[..]);
        tracing::info!("read_cur! {:?}", read_cur);
        let track_namespace_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("track namespace length")?;
        let track_namespace =
            read_variable_bytes_with_length_from_buffer(buf, track_namespace_length as usize)
                .context("track namespace")?;

        tracing::info!("track_namespace! {:?}", track_namespace);

        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;

        tracing::info!("number_of_parameters! {:?}", number_of_parameters);

        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let param = Parameter::depacketize(buf)?;
            parameters.push(param);
        }

        let announce_message = AnnounceMessage {
            track_namespace: String::from_utf8(track_namespace)?,
            number_of_parameters,
            parameters,
        };

        Ok(announce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            ANNOUNCE Message {
                Track Namespace(b), // (b): bytes Length and bytes
                Number of Parameters (i),
                Parameters (..) ...,
            }
        */

        // Track Namespace bytes Length
        buf.extend(write_variable_integer(self.track_namespace.len() as u64));
        // Track Namespace bytes
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        // Number of Parameters
        buf.extend(write_variable_integer(self.number_of_parameters as u64));
        // Parameters
        for param in &self.parameters {
            param.packetize(buf);
        }
    }
}

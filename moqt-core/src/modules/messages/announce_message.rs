use std::io::Cursor;

use anyhow::{Context, Result};

use crate::{
    modules::{
        messages::parameter::Parameter, variable_integer::read_variable_integer_from_buffer,
    },
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug)]
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
        let track_namespace =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("track namespace")?;
        let number_of_parameters = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of parameters")?;
        let mut parameters = vec![];
        for _ in 0..number_of_parameters {
            let param = Parameter::depacketize(buf)?;
            parameters.push(param);
        }

        let announce_message = AnnounceMessage {
            track_namespace,
            number_of_parameters,
            parameters,
        };
        tracing::info!("announce_message! {:?}", announce_message);

        Ok(announce_message)
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        /*
            ANNOUNCE Message {
                Track Namespace(b),
                Number of Parameters (i),
                Parameters (..) ...,
            }
        */

        // Track Namespace
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

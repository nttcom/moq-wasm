use std::{any::Any, io::Cursor};

use anyhow::{Context, Result};

use crate::{
    modules::{
        messages::version_specific_parameters::VersionSpecificParameter,
        variable_integer::read_variable_integer_from_buffer,
    },
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::write_variable_integer,
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, PartialEq)]
pub struct AnnounceMessage {
    pub(crate) track_namespace: String,
    pub(crate) number_of_parameters: u8,
    pub(crate) parameters: Vec<VersionSpecificParameter>,
}

impl AnnounceMessage {
    pub fn new(
        track_namespace: String,
        number_of_parameters: u8,
        parameters: Vec<VersionSpecificParameter>,
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
            let param = VersionSpecificParameter::depacketize(buf)?;
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
    /// Method to enable downcasting from MOQTPayload to AnnounceMessage
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::version_specific_parameters::AuthorizationInfo;
    use crate::modules::messages::announce_message::AnnounceMessage;
    use crate::modules::messages::version_specific_parameters::{
        VersionSpecificParameter, VersionSpecificParameterType,
    };
    use crate::modules::variable_integer::write_variable_integer;

    #[test]
    fn packetize_announce_with_parameter() {
        let track_namespace = "live.example.com/meeting/123/member/alice/".to_string();
        let number_of_parameters = 1;

        let parameter_value = "test".to_string();
        let parameter_length = parameter_value.len() as u8;

        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));
        let parameters = vec![parameter];
        let announce_message =
            AnnounceMessage::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        // Track Namespace Length
        let mut combined_bytes = Vec::from((track_namespace.len() as u8).to_be_bytes());
        // Track Namespace
        combined_bytes.extend(track_namespace.as_bytes().to_vec());
        // Number of Parameters
        combined_bytes.extend(1u8.to_be_bytes());
        // Parameters
        combined_bytes
            .extend((VersionSpecificParameterType::AuthorizationInfo as u8).to_be_bytes());
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        combined_bytes.extend(parameter_value.as_bytes());

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize_announce_with_parameter() {
        let track_namespace = "live.example.com/meeting/123/member/alice/".to_string();
        let number_of_parameters = 1;

        let parameter_value = "test".to_string();
        let parameter_length = parameter_value.len() as u8;

        let parameter = VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(
            parameter_value.clone(),
        ));
        let parameters = vec![parameter];
        let expected_announce_message =
            AnnounceMessage::new(track_namespace.clone(), number_of_parameters, parameters);

        // Track Namespace Length
        let mut combined_bytes = Vec::from((track_namespace.len() as u8).to_be_bytes());
        // Track Namespace
        combined_bytes.extend(track_namespace.as_bytes().to_vec());
        // Number of Parameters
        combined_bytes.extend(1u8.to_be_bytes());
        // Parameters
        combined_bytes
            .extend((VersionSpecificParameterType::AuthorizationInfo as u8).to_be_bytes());
        combined_bytes.extend(write_variable_integer(parameter_length as u64));
        combined_bytes.extend(parameter_value.as_bytes());

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_announce_message = AnnounceMessage::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_announce_message, expected_announce_message);
    }

    #[test]
    fn packetize_announce_without_parameter() {
        let track_namespace = "live.example.com/meeting/123/member/alice/".to_string();
        let number_of_parameters = 0;
        let parameters = vec![];
        let announce_message =
            AnnounceMessage::new(track_namespace.clone(), number_of_parameters, parameters);
        let mut buf = bytes::BytesMut::new();
        announce_message.packetize(&mut buf);

        // Track Namespace Length
        let mut combined_bytes = Vec::from((track_namespace.len() as u8).to_be_bytes());
        // Track Namespace
        combined_bytes.extend(track_namespace.as_bytes().to_vec());
        // Number of Parameters
        combined_bytes.extend(0u8.to_be_bytes());

        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize_announce_without_parameter() {
        let track_namespace = "live.example.com/meeting/123/member/alice/".to_string();
        let number_of_parameters = 0;

        let parameters = vec![];
        let expected_announce_message =
            AnnounceMessage::new(track_namespace.clone(), number_of_parameters, parameters);

        // Track Namespace Length
        let mut combined_bytes = Vec::from((track_namespace.len() as u8).to_be_bytes());
        // Track Namespace
        combined_bytes.extend(track_namespace.as_bytes().to_vec());
        // Number of Parameters
        combined_bytes.extend(0u8.to_be_bytes());

        let mut buf = bytes::BytesMut::from(combined_bytes.as_slice());
        let depacketized_announce_message = AnnounceMessage::depacketize(&mut buf).unwrap();

        assert_eq!(depacketized_announce_message, expected_announce_message);
    }
}

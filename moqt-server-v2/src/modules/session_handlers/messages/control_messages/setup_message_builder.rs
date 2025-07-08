use anyhow::bail;
use bytes::BytesMut;

use crate::modules::session_handlers::{
    constants,
    messages::{
        control_messages::{
            client_setup::ClientSetup,
            server_setup::ServerSetup,
            setup_parameters::{MaxSubscribeID, SetupParameter},
        },
        message_process_result::MessageProcessResult,
        moqt_payload::MOQTPayload,
    },
};

pub(crate) struct SetupMessageBuilder;

impl SetupMessageBuilder {
    const DOWNSTREAM_MAX_SUBSCRIBE_ID: u64 = 100;

    pub(crate) fn create_server_setup(payload_buffer: &mut BytesMut) -> MessageProcessResult {
        let _ = match ClientSetup::depacketize(payload_buffer) {
            Ok(client_setup_message) => client_setup_message,
            Err(err) => {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    constants::TerminationErrorCode::ProtocolViolation,
                    err.to_string(),
                );
            }
        };

        let server_setup_param = Self::create_setup_parameter();
        let server_setup_message =
            ServerSetup::new(constants::MOQ_TRANSPORT_VERSION, server_setup_param);
        let mut buffer = BytesMut::new();
        server_setup_message.packetize(&mut buffer);
        MessageProcessResult::Success(buffer)
    }

    fn create_setup_parameter() -> Vec<SetupParameter> {
        let mut setup_parameters = vec![];

        let max_subscribe_id_parameter =
            SetupParameter::MaxSubscribeID(MaxSubscribeID::new(Self::DOWNSTREAM_MAX_SUBSCRIBE_ID));
        setup_parameters.push(max_subscribe_id_parameter);

        setup_parameters
    }
}

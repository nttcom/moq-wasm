use bytes::BytesMut;

use crate::modules::session_handlers::{
    constants,
    messages::{
        control_messages::{
            client_setup::ClientSetup,
            server_setup::ServerSetup,
            setup_parameters::{MaxSubscribeID, SetupParameter},
        },
        moqt_payload::MOQTPayload,
    },
};

pub(crate) struct MOQTMessageBuilder;

impl MOQTMessageBuilder {
    const DOWNSTREAM_MAX_SUBSCRIBE_ID: u64 = 100;

    pub(crate) fn create_setup_parameter() -> Vec<SetupParameter> {
        let mut setup_parameters = vec![];

        let max_subscribe_id_parameter =
            SetupParameter::MaxSubscribeID(MaxSubscribeID::new(Self::DOWNSTREAM_MAX_SUBSCRIBE_ID));
        setup_parameters.push(max_subscribe_id_parameter);

        setup_parameters
    }

    pub(crate) fn client_setup(supported_versions: Vec<u32>, max_subscriber_id: u64) -> BytesMut {
        let _max_subscriber_id = if max_subscriber_id == 0 {
            Self::DOWNSTREAM_MAX_SUBSCRIBE_ID
        } else {
            max_subscriber_id
        };
        let mut buf = BytesMut::new();
        let mut setup_parameters = vec![];
        setup_parameters.push(SetupParameter::MaxSubscribeID(MaxSubscribeID::new(
            max_subscriber_id,
        )));
        ClientSetup::new(supported_versions, setup_parameters).packetize(&mut buf);

        buf
    }

    pub(crate) fn server_setup() -> BytesMut {
        let server_setup_param = Self::create_setup_parameter();
        let server_setup_message =
            ServerSetup::new(constants::MOQ_TRANSPORT_VERSION, server_setup_param);
        let mut buffer = BytesMut::new();
        server_setup_message.packetize(&mut buffer);
        buffer
    }
}

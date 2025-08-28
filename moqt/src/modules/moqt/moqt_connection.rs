use crate::modules::moqt::constants;
use crate::modules::moqt::messages::control_messages::setup_parameters::MaxSubscribeID;
use crate::modules::moqt::messages::control_messages::setup_parameters::SetupParameter;
use crate::modules::moqt::moqt_message_controller::MOQTMessageController;
use crate::modules::transport::transport_connection::TransportConnection;

pub(crate) struct MOQTConnection {
    transport_connection: Box<dyn TransportConnection>,
    message_controller: MOQTMessageController,
}

impl MOQTConnection {
    pub(crate) async fn new(
        is_client: bool,
        transport_connection: Box<dyn TransportConnection>,
        message_controller: MOQTMessageController,
    ) -> anyhow::Result<Self> {
        if is_client {
            Self::setup_for_client(&message_controller).await?;
        } else {
            Self::setup_for_server(&message_controller).await?;
        }
        Ok(Self {
            transport_connection,
            message_controller,
        })
    }

    async fn setup_for_client(message_controller: &MOQTMessageController) -> anyhow::Result<()> {
        let max_id = MaxSubscribeID::new(1000);
        message_controller
            .client_setup(
                vec![constants::MOQ_TRANSPORT_VERSION],
                vec![SetupParameter::MaxSubscribeID(max_id)],
            )
            .await
    }

    async fn setup_for_server(message_controller: &MOQTMessageController) -> anyhow::Result<()> {
        message_controller.server_setup().await
    }
}

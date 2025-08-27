use crate::modules::session_handlers::constants;
use crate::modules::session_handlers::messages::control_messages::setup_parameters::{
    MaxSubscribeID, SetupParameter,
};
use crate::modules::session_handlers::moqt_connection::MOQTConnection;
use crate::modules::session_handlers::moqt_message_controller::MOQTMessageController;
use crate::modules::session_handlers::transport_connection_creator::TransportConnectionCreator;

pub(crate) struct MOQTConnectionCreator {
    transport_creator: Box<dyn TransportConnectionCreator>,
}

impl MOQTConnectionCreator {
    pub(crate) fn new(transport_creator: Box<dyn TransportConnectionCreator>) -> Self {
        Self { transport_creator }
    }

    pub(crate) async fn create_new_connection(
        &self,
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<MOQTConnection> {
        let transport_conn = self
            .transport_creator
            .create_new_transport(server_name, port)
            .await?;
        let stream = transport_conn.accept_bi().await?;
        let message_controller = MOQTMessageController::new(stream);
        let max_id = MaxSubscribeID::new(1000);
        message_controller
            .client_setup(
                vec![constants::MOQ_TRANSPORT_VERSION],
                vec![SetupParameter::MaxSubscribeID(max_id)],
            )
            .await?;
        Ok(MOQTConnection::new(transport_conn, message_controller))
    }

    pub(crate) async fn accept_new_connection(&mut self) -> anyhow::Result<MOQTConnection> {
        let transport_conn = self.transport_creator.accept_new_transport().await?;
        let stream = transport_conn.accept_bi().await?;
        let message_controller = MOQTMessageController::new(stream);
        message_controller.server_setup().await?;
        Ok(MOQTConnection::new(transport_conn, message_controller))
    }
}

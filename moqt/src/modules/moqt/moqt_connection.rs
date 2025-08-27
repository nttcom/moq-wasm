use crate::modules::moqt::moqt_message_controller::MOQTMessageController;
use crate::modules::transport::transport_connection::TransportConnection;

pub(crate) struct MOQTConnection {
    transport_connection: Box<dyn TransportConnection>,
    message_controller: MOQTMessageController,
}

impl MOQTConnection {
    pub(crate) fn new(
        transport_connection: Box<dyn TransportConnection>,
        message_controller: MOQTMessageController,
    ) -> Self {
        Self {
            transport_connection,
            message_controller,
        }
    }
}

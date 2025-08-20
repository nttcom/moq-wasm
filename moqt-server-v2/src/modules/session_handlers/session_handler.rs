use crate::modules::session_handlers::{
    message_controller_trait::MessageControllerTrait, protocol_handler_trait::ConnectionCreator,
};

struct SessionHandler {
    protocol_handler: Box<dyn ConnectionCreator>,
    message_controller: Box<dyn MessageControllerTrait>,
}

impl SessionHandler {
    pub(crate) fn new(
        protocol_handler: Box<dyn ConnectionCreator>,
        message_controller: Box<dyn MessageControllerTrait>,
    ) -> Self {
        Self {
            protocol_handler,
            message_controller,
            stream_repo: Box::new(StreamRepository::new()),
        }
    }

    pub(crate) async fn connect(&mut self) -> anyhow::Result<()> {
        let stream = self.protocol_handler.start().await?;
        Ok(())
    }
}

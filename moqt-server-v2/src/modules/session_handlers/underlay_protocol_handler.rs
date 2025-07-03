use crate::modules::session_handlers::protocol_handler_trait::ProtocolHandlerTrait;

pub(crate) struct UnderlayProtocolHandler {
    handler: Box<dyn ProtocolHandlerTrait>,
}

impl UnderlayProtocolHandler {
    pub fn new(handler: Box<dyn ProtocolHandlerTrait>) -> Self {
        Self { handler }
    }

    pub async fn start(&self) {
        self.handler.start().await;
    }
}

use crate::modules::session_handlers::protocol_handler_trait::ProtocolHandlerTrait;

pub(crate) struct SessionHandler {
    _handler: Box<dyn ProtocolHandlerTrait>,
}

impl SessionHandler {
    pub fn new(handler: Box<dyn ProtocolHandlerTrait>) -> Self {
        Self { _handler: handler }
    }

    pub fn start(&self) {
        for i in 0.. {}
    }
}

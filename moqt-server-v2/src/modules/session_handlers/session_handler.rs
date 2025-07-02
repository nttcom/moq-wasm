use std::sync::Arc;

use crate::modules::server_processes::session_handlers::session_handler_trait::SessionHandlerTrait;

struct SessionHandler {
    _handler: Arc<dyn SessionHandlerTrait>,
}

impl SessionHandler {
    pub fn new(handler: Arc<dyn SessionHandlerTrait>) -> Self {
        Self { _handler: handler }
    }

    pub fn start(&self) {
        for i in 0.. {

        }
    }
}
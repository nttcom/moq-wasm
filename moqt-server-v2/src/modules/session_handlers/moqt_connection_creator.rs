use crate::modules::session_handlers::connection_creator::ConnectionCreator;

pub(crate) struct MOQTConnectionCreator {
    handler: Box<dyn ConnectionCreator>,
}

impl MOQTConnectionCreator {
    pub fn new(handler: Box<dyn ConnectionCreator>) -> Self {
        Self { handler }
    }

    pub async fn start(&mut self) {
        self.handler.start().await;
    }
}

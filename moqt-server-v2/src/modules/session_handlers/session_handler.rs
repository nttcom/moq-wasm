use crate::modules::session_handlers::moqt_connection_creator::MOQTConnectionFactory;

struct SessionHandler {
    creator: Box<dyn MOQTConnectionFactory>,
}

impl SessionHandler {
    pub(crate) fn new(creator: Box<dyn MOQTConnectionFactory>) -> Self {
        Self { creator }
    }

    pub(crate) async fn connect(&mut self) {
        self.creator.accept_new_connection().await;
    }
}

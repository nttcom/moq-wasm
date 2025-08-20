use crate::modules::session_handlers::{
    connection_trait::ConnectionTrait, moqt_bi_stream::MOQTBiStream,
};

pub(crate) struct MOQTConnection {
    connection: Box<dyn ConnectionTrait>,
}

impl MOQTConnection {
    pub(crate) fn new(connection: Box<dyn ConnectionTrait>) -> Self {
        Self { connection }
    }

    pub(crate) async fn accept_bi(&self) -> anyhow::Result<Box<dyn MOQTBiStream>> {
        self.connection.accept_bi().await
    }
}

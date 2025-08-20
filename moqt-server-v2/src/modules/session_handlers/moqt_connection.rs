use crate::modules::session_handlers::{
    connection::Connection, moqt_bi_stream::MOQTBiStream,
};

pub(crate) struct MOQTConnection {
    connection: Box<dyn Connection>,
}

impl MOQTConnection {
    pub(crate) fn new(connection: Box<dyn Connection>) -> Self {
        Self { connection }
    }

    pub(crate) async fn accept_bi(&self) -> anyhow::Result<Box<dyn MOQTBiStream>> {
        self.connection.accept_bi().await
    }
}

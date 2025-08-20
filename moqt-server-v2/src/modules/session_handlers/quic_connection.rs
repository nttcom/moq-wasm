use async_trait::async_trait;

use crate::modules::session_handlers::{
    connection::Connection, moqt_bi_stream::MOQTBiStream, quic_bi_stream::QuicBiStream,
};

pub(crate) struct QuicConnection {
    connection: quinn::Connection,
}

impl QuicConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl Connection for QuicConnection {
    async fn accept_bi(&self) -> anyhow::Result<Box<dyn MOQTBiStream>> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        let stream = QuicBiStream::new(
            self.connection.stable_id(),
            receiver.id().into(),
            receiver,
            sender,
        );
        Ok(Box::new(stream))
    }
}

use std::sync::Arc;

use async_trait::async_trait;

use crate::modules::moqt::moqt_bi_stream::MOQTBiStream;
use crate::modules::transport::quic::quic_bi_stream::QUICBiStream;
use crate::modules::transport::transport_connection::TransportConnection;

pub(crate) struct QUICConnection {
    connection: quinn::Connection,
}

impl QUICConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl TransportConnection for QUICConnection {
    async fn open_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn MOQTBiStream>>> {
        let (sender, receiver) = self.connection.open_bi().await?;
        let stream = QUICBiStream::new(
            self.connection.stable_id(),
            receiver.id().into(),
            receiver,
            sender,
        );
        Ok(Arc::new(tokio::sync::Mutex::new(stream)))
    }

    async fn accept_bi(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<dyn MOQTBiStream>>> {
        let (sender, receiver) = self.connection.accept_bi().await?;
        let stream = QUICBiStream::new(
            self.connection.stable_id(),
            receiver.id().into(),
            receiver,
            sender,
        );
        Ok(Arc::new(tokio::sync::Mutex::new(stream)))
    }
}

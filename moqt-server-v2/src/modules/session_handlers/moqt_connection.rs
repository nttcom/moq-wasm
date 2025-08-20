use async_trait::async_trait;

use crate::modules::session_handlers::moqt_bi_stream::MOQTBiStream;

#[async_trait]
pub(crate) trait MOQTConnection {
    async fn accept_bi(&self) -> anyhow::Result<Box<dyn MOQTBiStream>>;
}

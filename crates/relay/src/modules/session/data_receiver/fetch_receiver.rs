#[async_trait::async_trait]
pub(crate) trait UpstreamFetchReceiver: Send + 'static {
    async fn receive(&mut self) -> anyhow::Result<moqt::Fetch>;
}

#[async_trait::async_trait]
impl UpstreamFetchReceiver for moqt::FetchDataReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::Fetch> {
        self.receive().await
    }
}

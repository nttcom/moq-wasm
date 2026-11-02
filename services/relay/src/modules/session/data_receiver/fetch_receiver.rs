#[async_trait::async_trait]
pub(crate) trait UpstreamFetchReceiver: Send + 'static {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch>;
}

#[async_trait::async_trait]
impl UpstreamFetchReceiver for moqt::fetch::FetchDataReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch> {
        self.receive().await
    }
}

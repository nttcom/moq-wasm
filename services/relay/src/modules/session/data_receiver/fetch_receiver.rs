#[async_trait::async_trait]
pub(crate) trait UpstreamFetchReceiver: Send + 'static {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch>;

    /// `false` for a draft-14 response, whose datagram objects arrive in
    /// the subgroup of their Object ID.
    fn carries_forwarding_preference(&self) -> bool;

    /// `false` for a draft-18 response, whose duplicates may differ in
    /// status.
    fn compares_duplicate_status(&self) -> bool;
}

#[async_trait::async_trait]
impl UpstreamFetchReceiver for moqt::fetch::FetchDataReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch> {
        self.receive().await
    }

    fn carries_forwarding_preference(&self) -> bool {
        moqt::fetch::FetchDataReceiver::carries_forwarding_preference(self)
    }

    fn compares_duplicate_status(&self) -> bool {
        moqt::fetch::FetchDataReceiver::compares_duplicate_status(self)
    }
}

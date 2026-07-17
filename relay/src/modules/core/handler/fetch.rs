use async_trait::async_trait;

#[async_trait]
pub(crate) trait FetchHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn group_order(&self) -> moqt::GroupOrder;
    fn fetch_params(&self) -> moqt::wire::FetchParams;
    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::Location,
    ) -> Result<(), moqt::TransportSendError>;
    async fn error(&self, code: u64, reason: String) -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> FetchHandler for moqt::FetchHandler<T> {
    fn request_id(&self) -> u64 {
        self.request_id
    }

    fn group_order(&self) -> moqt::GroupOrder {
        self.fetch.group_order
    }

    fn fetch_params(&self) -> moqt::wire::FetchParams {
        self.fetch.fetch_params.clone()
    }

    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::Location,
    ) -> Result<(), moqt::TransportSendError> {
        self.ok(end_of_track, end_location).await
    }

    async fn error(&self, code: u64, reason: String) -> Result<(), moqt::TransportSendError> {
        self.error(code, reason).await
    }
}

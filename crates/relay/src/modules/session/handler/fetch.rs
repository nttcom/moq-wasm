use async_trait::async_trait;
use moqt::RequestErrorCode;

#[async_trait]
pub(crate) trait FetchHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn group_order(&self) -> moqt::object::GroupOrder;
    fn fetch_params(&self) -> moqt::fetch::FetchParams;
    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::object::Location,
    ) -> Result<(), moqt::TransportSendError>;
    async fn error(
        &self,
        code: RequestErrorCode,
        reason: String,
    ) -> Result<(), moqt::TransportSendError>;
}

#[async_trait]
impl FetchHandler for moqt::fetch::FetchHandler {
    fn request_id(&self) -> u64 {
        self.request_id
    }

    fn group_order(&self) -> moqt::object::GroupOrder {
        self.fetch.group_order
    }

    fn fetch_params(&self) -> moqt::fetch::FetchParams {
        self.fetch.fetch_params.clone()
    }

    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::object::Location,
    ) -> Result<(), moqt::TransportSendError> {
        self.ok(end_of_track, end_location).await
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        reason: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.error(code, reason).await
    }
}

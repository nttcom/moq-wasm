use async_trait::async_trait;
use moqt::{
    RequestErrorCode, TrackProperties,
    fetch::{JoinedSubscription, JoiningFetchRefusal},
};

#[async_trait]
pub(crate) trait FetchHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn group_order(&self) -> moqt::object::GroupOrder;
    fn fetch_params(&self) -> moqt::fetch::FetchParams;
    fn check_joining(&self, subscription: JoinedSubscription) -> Result<(), JoiningFetchRefusal>;
    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::object::Location,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()>;
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

    fn check_joining(&self, subscription: JoinedSubscription) -> Result<(), JoiningFetchRefusal> {
        moqt::fetch::FetchHandler::check_joining(self, subscription)
    }

    async fn ok(
        &self,
        end_of_track: bool,
        end_location: moqt::object::Location,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()> {
        // moqt sends the handler's `track_properties` field; the clone shares
        // the original's response guard.
        let mut handler = self.clone();
        handler.track_properties = track_properties;
        moqt::fetch::FetchHandler::ok(&handler, end_of_track, end_location).await
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        reason: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.error(code, reason).await
    }
}

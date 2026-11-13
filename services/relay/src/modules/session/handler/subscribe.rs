use async_trait::async_trait;
use moqt::{RequestErrorCode, TrackProperties};
use moqt::{
    object::{ContentExists, GroupOrder},
    subscribe::FilterType,
};

use crate::logging::mark_current_span_failed;
use crate::modules::session::subscription::DownstreamSubscription;

#[async_trait]
pub(crate) trait SubscribeHandler: 'static + Send + Sync {
    fn trace_context(&self) -> Option<&moqt::TraceContext>;
    fn subscribe_id(&self) -> u64;
    fn track_namespace(&self) -> &str;
    fn track_namespace_tuple(&self) -> &[String];
    fn track_name(&self) -> &str;
    fn _subscriber_priority(&self) -> u8;
    fn _group_order(&self) -> GroupOrder;
    fn forward(&self) -> bool;
    fn _filter_type(&self) -> FilterType;
    fn new_group_request(&self) -> Option<u64>;
    /// The DELIVERY TIMEOUT parameter in milliseconds.
    fn delivery_timeout(&self) -> Option<u64>;
    fn allocate_track_alias(&self) -> u64;
    fn understands(&self, track_properties: &TrackProperties) -> bool;
    async fn ok_with_track_alias(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()>;
    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError>;
    fn to_downstream_subscription(&self, track_alias: u64) -> DownstreamSubscription;
}

#[async_trait]
impl SubscribeHandler for moqt::subscribe::SubscribeHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        moqt::subscribe::SubscribeHandler::trace_context(self)
    }
    fn subscribe_id(&self) -> u64 {
        self.request_id()
    }
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }
    fn track_name(&self) -> &str {
        &self.track_name
    }
    fn _subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }
    fn _group_order(&self) -> GroupOrder {
        self.group_order
    }
    fn forward(&self) -> bool {
        self.forward
    }
    fn _filter_type(&self) -> FilterType {
        self.filter_type
    }
    fn new_group_request(&self) -> Option<u64> {
        self.new_group_request
    }
    fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }
    fn allocate_track_alias(&self) -> u64 {
        moqt::subscribe::SubscribeHandler::allocate_track_alias(self)
    }
    fn understands(&self, track_properties: &TrackProperties) -> bool {
        moqt::subscribe::SubscribeHandler::understands(self, track_properties)
    }

    async fn ok_with_track_alias(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()> {
        // moqt sends the handler's `track_properties` field; the clone shares
        // the original's response guard.
        let mut handler = self.clone();
        handler.track_properties = track_properties;
        moqt::subscribe::SubscribeHandler::ok_with_track_alias(
            &handler,
            track_alias,
            expires,
            content_exists,
        )
        .await
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        mark_current_span_failed(format!("{code:?}"));
        self.error(code, reason_phrase).await
    }

    fn to_downstream_subscription(&self, track_alias: u64) -> DownstreamSubscription {
        DownstreamSubscription::from(self.into_subscription(track_alias))
    }
}

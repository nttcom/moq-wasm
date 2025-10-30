use std::sync::Arc;

use crate::{
    DatagramSender, FilterType, GroupOrder, SubscribeHandler, SubscriberPriority,
    TransportProtocol,
    modules::{
        moqt::{
            messages::control_messages::{location::Location, publish_ok::PublishOk},
            sessions::session_context::SessionContext,
            streams::stream::stream_sender::StreamSender,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct Publication<T: TransportProtocol> {
    pub(crate) session_context: Arc<SessionContext<T>>,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub subscriber_priority: SubscriberPriority,
    pub forward: bool,
    pub filter_type: FilterType,
    pub start_location: Option<Location>,
    pub end_group: Option<u64>,
    pub delivery_timeout: Option<u64>,
}

impl<T: TransportProtocol> Publication<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
        publish_ok: PublishOk,
    ) -> Self {
        Self {
            session_context,
            track_namespace,
            track_name,
            track_alias,
            group_order: publish_ok.group_order,
            subscriber_priority: publish_ok.subscriber_priority,
            forward: publish_ok.forward,
            filter_type: publish_ok.filter_type,
            start_location: publish_ok.start_location,
            end_group: publish_ok.end_group,
            delivery_timeout: None,
        }
    }

    pub(crate) fn from_subscribe_handler(
        session_context: Arc<SessionContext<T>>,
        track_alias: u64,
        handler: &SubscribeHandler<T>,
    ) -> Self {
        Self {
            session_context,
            track_namespace: handler.track_namespace.clone(),
            track_name: handler.track_name.clone(),
            track_alias,
            group_order: handler.group_order,
            subscriber_priority: handler.subscriber_priority,
            forward: handler.forward,
            filter_type: handler.filter_type,
            start_location: handler.start_location,
            end_group: handler.end_group,
            delivery_timeout: None,
        }
    }

    pub async fn create_stream(&self) -> anyhow::Result<StreamSender<T>> {
        let send_stream = self.session_context.transport_connection.open_uni().await?;
        Ok(StreamSender::new(send_stream))
    }

    pub fn create_datagram(&self, track_alias: u64) -> DatagramSender<T> {
        DatagramSender::new(track_alias, self.session_context.clone())
    }
}

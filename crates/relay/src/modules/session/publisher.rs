use std::{future::Future, pin::Pin};

use async_trait::async_trait;
use moqt::ContentExists;

use crate::modules::session::{
    data_sender::{
        DataSender,
        fetch_sender::FetchSender,
        stream_sender_factory::{ConcreteStreamSenderFactory, StreamSenderFactory},
    },
    subscription::DownstreamSubscription,
};

pub(crate) type PublishNamespaceResponse = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(
        &self,
        namespaces: String,
    ) -> anyhow::Result<PublishNamespaceResponse>;
    async fn send_publish_namespace_done(&self, namespace: String) -> anyhow::Result<()>;
    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
    ) -> anyhow::Result<DownstreamSubscription>;
    async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()>;
    fn new_stream_factory(
        &self,
        downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory>;
    fn new_datagram(&self, downstream_subscription: &DownstreamSubscription)
    -> Box<dyn DataSender>;
    async fn new_fetch_sender(&self, request_id: u64) -> anyhow::Result<Box<dyn FetchSender>>;
}

#[async_trait]
impl Publisher for moqt::Publisher {
    async fn send_publish_namespace(
        &self,
        namespaces: String,
    ) -> anyhow::Result<PublishNamespaceResponse> {
        let pending = self.begin_publish_namespace(namespaces).await?;
        Ok(Box::pin(pending.accepted()))
    }

    async fn send_publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        self.publish_namespace_done(namespace).await
    }

    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
    ) -> anyhow::Result<DownstreamSubscription> {
        let option = moqt::PublishOption {
            content_exists,
            ..Default::default()
        };
        let subscription = self.publish(track_namespace, track_name, option).await?;
        Ok(DownstreamSubscription::from(subscription))
    }

    async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        self.publish_done(request_id, status_code, stream_count, error_reason)
            .await
    }

    fn new_stream_factory(
        &self,
        downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        let subscriber_track_alias = downstream_subscription.track_alias();
        let inner = self.create_stream(downstream_subscription.as_moqt());
        Box::new(ConcreteStreamSenderFactory::new(
            inner,
            subscriber_track_alias,
        ))
    }

    fn new_datagram(
        &self,
        downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DataSender> {
        Box::new(self.create_datagram(downstream_subscription.as_moqt()))
    }

    async fn new_fetch_sender(&self, request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        let sender = self.create_fetch_stream(request_id).await?;
        Ok(Box::new(sender))
    }
}

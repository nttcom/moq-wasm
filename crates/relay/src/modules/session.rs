pub(crate) mod data_object;
pub(crate) mod data_receiver;
pub(crate) mod data_sender;
pub(crate) mod handler;
pub(crate) mod moqt_session_event;
pub(crate) mod publisher;
pub(crate) mod session_event;
pub(crate) mod session_repository;
pub(crate) mod subscriber;
pub(crate) mod subscription;

use async_trait::async_trait;

use crate::modules::session::{
    moqt_session_event::MoqtSessionEvent, publisher::Publisher, subscriber::Subscriber,
};

#[async_trait]
pub(crate) trait Session: 'static + Send + Sync {
    fn as_publisher(&self) -> Box<dyn Publisher>;
    fn as_subscriber(&self) -> Box<dyn Subscriber>;
    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent>;
    fn close(&self, code: moqt::TerminationErrorCode, reason: &str);
    fn transport_stats(&self) -> moqt::TransportStats;
    fn transport_addresses(&self) -> moqt::TransportAddresses;
}

#[async_trait]
impl Session for moqt::Session {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        Box::new(self.publisher())
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(self.subscriber())
    }

    fn close(&self, code: moqt::TerminationErrorCode, reason: &str) {
        self.close_with_error(code, reason);
    }

    fn transport_stats(&self) -> moqt::TransportStats {
        moqt::Session::transport_stats(self)
    }

    fn transport_addresses(&self) -> moqt::TransportAddresses {
        moqt::Session::transport_addresses(self)
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        let event = self.receive_event().await?;
        let result = match event {
            moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                MoqtSessionEvent::PublishNamespace(Box::new(publish_namespace_handler))
            }
            moqt::SessionEvent::PublishNamespaceDone(publish_namespace_done_handler) => {
                MoqtSessionEvent::PublishNamespaceDone(publish_namespace_done_handler)
            }
            moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                MoqtSessionEvent::SubscribeNamespace(Box::new(subscribe_namespace_handler))
            }
            moqt::SessionEvent::UnsubscribeNamespace(unsubscribe_namespace_handler) => {
                MoqtSessionEvent::UnsubscribeNamespace(unsubscribe_namespace_handler)
            }
            moqt::SessionEvent::Publish(publish_handler) => {
                MoqtSessionEvent::Publish(Box::new(publish_handler))
            }
            moqt::SessionEvent::Subscribe(subscribe_handler) => {
                MoqtSessionEvent::Subscribe(Box::new(subscribe_handler))
            }
            moqt::SessionEvent::Unsubscribe(unsubscribe_handler) => {
                MoqtSessionEvent::Unsubscribe(Box::new(unsubscribe_handler))
            }
            moqt::SessionEvent::Disconnected() => MoqtSessionEvent::Disconnected(),
            moqt::SessionEvent::ProtocolViolation() => MoqtSessionEvent::ProtocolViolation(),
            moqt::SessionEvent::Fetch(fetch_handler) => {
                MoqtSessionEvent::Fetch(Box::new(fetch_handler))
            }
            moqt::SessionEvent::FetchCancel(fetch_cancel_handler) => {
                MoqtSessionEvent::FetchCancel(fetch_cancel_handler)
            }
            moqt::SessionEvent::GoAway(go_away_handler) => {
                MoqtSessionEvent::GoAway(go_away_handler)
            }
            moqt::SessionEvent::MaxRequestId(max_request_id_handler) => {
                MoqtSessionEvent::MaxRequestId(max_request_id_handler)
            }
            moqt::SessionEvent::RequestsBlocked(requests_blocked_handler) => {
                MoqtSessionEvent::RequestsBlocked(requests_blocked_handler)
            }
            moqt::SessionEvent::PublishDone(publish_done_handler) => {
                MoqtSessionEvent::PublishDone(publish_done_handler)
            }
            moqt::SessionEvent::PublishNamespaceCancel(publish_namespace_cancel_handler) => {
                MoqtSessionEvent::PublishNamespaceCancel(publish_namespace_cancel_handler)
            }
            moqt::SessionEvent::SubscribeUpdate(subscribe_update_handler) => {
                MoqtSessionEvent::SubscribeUpdate(subscribe_update_handler)
            }
            moqt::SessionEvent::TrackStatus(track_status_handler) => {
                MoqtSessionEvent::TrackStatus(Box::new(track_status_handler))
            }
        };
        Ok(result)
    }
}

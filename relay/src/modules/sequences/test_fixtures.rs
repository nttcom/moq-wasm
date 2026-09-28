use moqt::ContentExists;
use tokio::sync::mpsc;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::mocks::{RecordedControlMessages, session_repository_with_upstream_session},
    relay::ingress::ingress_coordinator::IngressCommand,
    sequences::tables::{
        hashmap_table::InMemoryLocalPubSubDirectory,
        table::{ActiveUpstreamSubscription, UpstreamSubscriptionKey, UpstreamSubscriptionOrigin},
    },
    types::{SessionId, TrackKey},
};

pub(crate) const PUBLISHER_SESSION: SessionId = 1;
pub(crate) const UPSTREAM_REQUEST_ID: u64 = 42;

pub(crate) fn upstream_key() -> UpstreamSubscriptionKey {
    UpstreamSubscriptionKey {
        publisher_session_id: PUBLISHER_SESSION,
        track_namespace: "ns".to_string(),
        track_name: "track".to_string(),
    }
}

pub(crate) fn active_upstream(origin: UpstreamSubscriptionOrigin) -> ActiveUpstreamSubscription {
    ActiveUpstreamSubscription {
        upstream_request_id: UPSTREAM_REQUEST_ID,
        track_key: TrackKey::new("ns", "track"),
        expires: None,
        content_exists: ContentExists::False,
        downstream_subscriber_count: 0,
        origin,
    }
}

pub(crate) fn table_with_upstream(
    origin: UpstreamSubscriptionOrigin,
) -> (InMemoryLocalPubSubDirectory, UpstreamSubscriptionKey) {
    let table = InMemoryLocalPubSubDirectory::new();
    table.register_upstream_subscription(upstream_key(), active_upstream(origin));
    (table, upstream_key())
}

pub(crate) struct UpstreamReleaseContext {
    pub(crate) table: InMemoryLocalPubSubDirectory,
    pub(crate) upstream_key: UpstreamSubscriptionKey,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) ingress_sender: mpsc::Sender<IngressCommand>,
    pub(crate) ingress_receiver: mpsc::Receiver<IngressCommand>,
    pub(crate) recorded: RecordedControlMessages,
}

pub(crate) async fn upstream_release_context(
    origin: UpstreamSubscriptionOrigin,
) -> UpstreamReleaseContext {
    let (table, upstream_key) = table_with_upstream(origin);
    let (repository, recorded) = session_repository_with_upstream_session(PUBLISHER_SESSION).await;
    let (ingress_sender, ingress_receiver) = mpsc::channel(8);
    UpstreamReleaseContext {
        table,
        upstream_key,
        forwarder: ControlMessageForwarder { repository },
        ingress_sender,
        ingress_receiver,
        recorded,
    }
}

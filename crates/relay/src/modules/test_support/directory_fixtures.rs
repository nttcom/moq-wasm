use moqt::ContentExists;
use tokio::sync::mpsc;

use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{
                ActiveUpstreamSubscription, UpstreamSubscriptionKey, UpstreamSubscriptionOrigin,
                UpstreamTrack,
            },
        },
        session_id::SessionId,
        track_key::TrackKey,
    },
    test_support::mock_session::{
        RecordedControlMessages, session_repository_with_upstream_session,
    },
};

pub(crate) const PUBLISHER_SESSION: SessionId = 1;
pub(crate) const UPSTREAM_REQUEST_ID: u64 = 42;

pub(crate) fn track_key() -> TrackKey {
    TrackKey::new("ns", "track")
}

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
        expires: None,
        content_exists: ContentExists::False,
        origin,
    }
}

pub(crate) fn upstream_track(origin: UpstreamSubscriptionOrigin) -> UpstreamTrack {
    let mut upstream_track = UpstreamTrack::default();
    upstream_track
        .subscriptions
        .insert(PUBLISHER_SESSION, active_upstream(origin));
    upstream_track
}

pub(crate) fn table_with_upstream(
    origin: UpstreamSubscriptionOrigin,
) -> (InMemoryLocalPubSubDirectory, TrackKey) {
    let table = InMemoryLocalPubSubDirectory::new();
    table.register_upstream_subscription(track_key(), PUBLISHER_SESSION, active_upstream(origin));
    (table, track_key())
}

pub(crate) struct UpstreamReleaseContext {
    pub(crate) table: InMemoryLocalPubSubDirectory,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) ingress_sender: mpsc::Sender<IngressCommand>,
    pub(crate) ingress_receiver: mpsc::Receiver<IngressCommand>,
    pub(crate) recorded: RecordedControlMessages,
}

pub(crate) async fn upstream_release_context(
    origin: UpstreamSubscriptionOrigin,
) -> UpstreamReleaseContext {
    let (table, _) = table_with_upstream(origin);
    let (repository, recorded) = session_repository_with_upstream_session(PUBLISHER_SESSION).await;
    let (ingress_sender, ingress_receiver) = mpsc::channel(8);
    UpstreamReleaseContext {
        table,
        forwarder: ControlMessageForwarder { repository },
        ingress_sender,
        ingress_receiver,
        recorded,
    }
}

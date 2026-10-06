use std::sync::Arc;

use tokio::{
    sync::mpsc::{Sender, UnboundedSender},
    task::JoinHandle,
};
use tracing::Instrument;

use super::{Fetch, FetchTarget};
use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressFetchRequest},
        ingress::fetch_ingest::{FetchIngest, FetchIngestStart},
    },
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
    session::{handler::fetch::FetchHandler, session_event::SessionEvent},
};

pub(super) struct UpstreamFetchStart {
    pub(super) session_id: SessionId,
    pub(super) handler: Box<dyn FetchHandler>,
    pub(super) target: FetchTarget,
    pub(super) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(super) forwarder: ControlMessageForwarder,
    pub(super) upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
    pub(super) cache_store: Arc<TrackCacheStore>,
    pub(super) egress_sender: Sender<EgressCommand>,
    pub(super) session_event_sender: UnboundedSender<SessionEvent>,
}

/// Off the session worker so a publisher that never answers FETCH does not stall the session.
pub(super) struct UpstreamFetchTask {
    _join_handle: JoinHandle<()>,
}

impl UpstreamFetchTask {
    pub(super) fn run(start: UpstreamFetchStart) -> Self {
        let join_handle = tokio::spawn(Self::forward(start).instrument(tracing::Span::current()));
        Self {
            _join_handle: join_handle,
        }
    }

    async fn forward(start: UpstreamFetchStart) {
        let Some(prepared) = Fetch::create_upstream_fetch(
            &start.table,
            &start.forwarder,
            &start.upstream_publisher_resolver,
            start.handler.as_ref(),
            &start.target,
        )
        .await
        else {
            return;
        };

        if let Err(e) = start
            .handler
            .ok(prepared.handle.end_of_track, prepared.handle.end_location)
            .await
        {
            tracing::error!(?e, "Failed to send FETCH_OK to downstream");
            return;
        }

        let egress_start = EgressFetchRequest {
            subscriber_session_id: start.session_id,
            request_id: start.handler.request_id(),
            cache: start.cache_store.get_or_create(&start.target.track_key),
            start_location: start.target.start_location,
            end_location: prepared.handle.end_location,
            group_order: start.handler.group_order(),
        };
        let _fetch_ingest = FetchIngest::run(
            start.forwarder.repository.clone(),
            start.egress_sender,
            start.session_event_sender,
            FetchIngestStart {
                track_key: start.target.track_key,
                upstream_publisher_session_id: prepared.upstream_publisher_session_id,
                fetch_handle: prepared.handle,
                egress_start,
            },
        );
    }
}

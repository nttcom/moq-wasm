use std::sync::Arc;

use tokio::{
    sync::mpsc::{Sender, UnboundedSender},
    task::JoinHandle,
};
use tracing::Instrument;

use super::{Fetch, FetchTarget};
use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::fetch::FetchHandler,
    relay::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressFetchRequest},
        ingress::fetch_ingest::{FetchIngest, FetchIngestStart},
    },
    sequences::tables::hashmap_table::InMemoryLocalPubSubDirectory,
    session_event::SessionEvent,
    types::SessionId,
    upstream_publisher_resolver::UpstreamPublisherResolver,
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

/// Waits for the upstream FETCH_OK outside the session worker, so a publisher
/// that answers late or never does not hold the requesting session's later
/// requests. The downstream FETCH_OK and the cache fill follow once it arrives.
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
        let UpstreamFetchStart {
            session_id,
            handler,
            target,
            table,
            forwarder,
            upstream_publisher_resolver,
            cache_store,
            egress_sender,
            session_event_sender,
        } = start;
        let Some(prepared) = Fetch::create_upstream_fetch(
            &table,
            &forwarder,
            &upstream_publisher_resolver,
            handler.as_ref(),
            &target,
        )
        .await
        else {
            return;
        };

        if let Err(e) = handler
            .ok(prepared.handle.end_of_track, prepared.handle.end_location)
            .await
        {
            tracing::error!(?e, "Failed to send FETCH_OK to downstream");
            return;
        }

        let egress_start = EgressFetchRequest {
            subscriber_session_id: session_id,
            request_id: handler.request_id(),
            cache: cache_store.get_or_create(&target.track_key),
            start_location: target.start_location,
            end_location: prepared.handle.end_location,
            group_order: handler.group_order(),
        };
        let _fetch_ingest = FetchIngest::run(
            forwarder.repository.clone(),
            egress_sender,
            session_event_sender,
            FetchIngestStart {
                track_key: target.track_key.clone(),
                upstream_publisher_session_id: prepared.upstream_publisher_session_id,
                fetch_handle: prepared.handle,
                egress_start,
            },
        );
    }
}

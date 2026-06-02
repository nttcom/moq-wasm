use tracing::Span;

use crate::modules::{
    core::handler::fetch::FetchHandler,
    relay::egress::coordinator::{EgressCommand, EgressFetchRequest},
    sequences::tables::table::LocalPubSubDirectory,
    types::SessionId,
};

pub(crate) struct Fetch;

impl Fetch {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.fetch",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn LocalPubSubDirectory,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn FetchHandler>,
    ) {
        let (track_namespace, track_name, start_group, start_object, end_group, end_object) =
            match handler.fetch_type() {
                // TODO: implement relative/absolute joining fetch.
                moqt::wire::FetchType::Standalone {
                    track_namespace,
                    track_name,
                    start_location,
                    end_location,
                } => (
                    track_namespace.join("/"),
                    track_name,
                    start_location.group_id,
                    start_location.object_id,
                    end_location.group_id,
                    end_location.object_id,
                ),
                moqt::wire::FetchType::RelativeJoining { .. }
                | moqt::wire::FetchType::AbsoluteJoining { .. } => {
                    tracing::warn!("Joining fetch is not yet implemented");
                    // 0 = Internal Error (TODO: use proper FetchErrorCode enum)
                    let _ = handler
                        .error(0, "Joining fetch is not yet implemented".to_string())
                        .await;
                    return;
                }
            };

        // FIXME: Fetch fails if publisher has disconnected, even if cached data exists.
        // Root cause: active_upstream_subscriptions are cleared on publisher disconnect,
        // so track_key cannot be resolved after that.
        let active_upstream_keys =
            table.find_active_upstream_subscriptions(&track_namespace, &track_name);
        let Some(key) = active_upstream_keys.into_iter().next() else {
            tracing::warn!(
                track_namespace,
                track_name,
                "No active upstream subscription found for fetch"
            );
            // TODO: use proper FetchErrorCode enum. 0x4 = TRACK_DOES_NOT_EXIST
            let _ = handler.error(0x4, "Track not found".to_string()).await;
            return;
        };
        let Some(active_upstream) = table.get_active_upstream_subscription(
            key.publisher_session_id,
            &track_namespace,
            &track_name,
        ) else {
            tracing::warn!(
                track_namespace,
                track_name,
                "Active upstream subscription data not found"
            );
            let _ = handler.error(0x4, "Track not found".to_string()).await;
            return;
        };
        let track_key = active_upstream.track_key;

        // Send FETCH_OK on the control stream.
        // FIXME: end_location should reflect the last object actually available in cache,
        // not the requested end.
        let end_location = moqt::Location {
            group_id: end_group,
            object_id: end_object,
        };

        if let Err(e) = handler.ok(false, end_location).await {
            tracing::error!(?e, "Failed to send FETCH_OK");
            return;
        }

        // Delegate data delivery to egress.
        let fetch_request = EgressFetchRequest {
            subscriber_session_id: session_id,
            request_id: handler.request_id(),
            track_key,
            start_location: moqt::Location {
                group_id: start_group,
                object_id: start_object,
            },
            end_location,
        };
        if let Err(e) = egress_sender
            .send(EgressCommand::StartFetch(fetch_request))
            .await
        {
            tracing::error!(?e, "Failed to send fetch request to egress");
        }
    }
}

use moqt::RequestFailure;
use tokio::task::JoinHandle;
use tracing::Instrument;

use super::DownstreamPublish;
use crate::modules::domain::{session_id::SessionId, track_key::TrackKey};

/// Off the session worker so a namespace subscriber that answers PUBLISH late or never does not
/// stall the session.
pub(crate) struct DownstreamPublishTask {
    _join_handle: JoinHandle<()>,
}

impl DownstreamPublishTask {
    pub(crate) fn run(
        downstream_publish: DownstreamPublish,
        subscriber_session_id: SessionId,
        track_key: TrackKey,
    ) -> Self {
        let join_handle = tokio::spawn(
            async move {
                match downstream_publish
                    .send(subscriber_session_id, &track_key)
                    .await
                {
                    // moqt logs the PUBLISH_OK or PUBLISH_ERROR at INFO.
                    Ok(()) => tracing::debug!(
                        subscriber_session_id = %subscriber_session_id,
                        "forwarded PUBLISH accepted"
                    ),
                    Err(error)
                        if matches!(
                            error.downcast_ref::<RequestFailure>(),
                            Some(RequestFailure::Rejected { .. })
                        ) =>
                    {
                        tracing::debug!(
                            %error,
                            subscriber_session_id = %subscriber_session_id,
                            "forwarded PUBLISH rejected"
                        )
                    }
                    Err(error) => tracing::warn!(
                        %error,
                        subscriber_session_id = %subscriber_session_id,
                        track_namespace = %track_key.track_namespace,
                        track_name = %track_key.track_name,
                        "forwarded PUBLISH failed"
                    ),
                }
            }
            .in_current_span(),
        );
        Self {
            _join_handle: join_handle,
        }
    }
}

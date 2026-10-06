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
                    Ok(()) => tracing::info!(
                        subscriber_session_id = %subscriber_session_id,
                        "forwarded PUBLISH accepted"
                    ),
                    Err(error) => tracing::warn!(
                        ?error,
                        subscriber_session_id = %subscriber_session_id,
                        "forwarded PUBLISH not accepted"
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

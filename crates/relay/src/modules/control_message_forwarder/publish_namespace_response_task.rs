use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::modules::core::publisher::PublishNamespaceResponse;

pub(super) struct PublishNamespaceResponseTask {
    _join_handle: JoinHandle<()>,
}

impl PublishNamespaceResponseTask {
    pub(super) fn run(response: PublishNamespaceResponse) -> Self {
        let join_handle = tokio::spawn(
            async move {
                match response.await {
                    Ok(()) => tracing::debug!("PUBLISH_NAMESPACE accepted"),
                    Err(error) => tracing::warn!(?error, "PUBLISH_NAMESPACE not accepted"),
                }
            }
            .in_current_span(),
        );
        Self {
            _join_handle: join_handle,
        }
    }
}

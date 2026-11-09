use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::modules::{
    data_plane::cache::track_cache::{FetchCursor, FetchInterrupted, TrackCache},
    domain::session_id::SessionId,
    session::{data_sender::fetch_sender::FetchSender, publisher::Publisher},
};
use moqt::object::DataStreamResetCode;

pub(crate) struct EgressFetchRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) start_location: moqt::object::Location,
    pub(crate) end_location: moqt::object::Location,
    pub(crate) group_order: moqt::object::GroupOrder,
}

/// Serves one FETCH from the cache over a new fetch stream of `publisher`.
pub(crate) struct FetchDeliveryTask {
    _join_handle: JoinHandle<()>,
}

impl FetchDeliveryTask {
    pub(crate) fn run(publisher: Box<dyn Publisher>, request: EgressFetchRequest) -> Self {
        let join_handle = tokio::spawn(
            async move {
                let sender = match publisher.new_fetch_sender(request.request_id).await {
                    Ok(sender) => sender,
                    Err(error) => {
                        tracing::error!(?error, "failed to create fetch sender");
                        return;
                    }
                };
                deliver_fetch(&request, sender.as_ref()).await;
            }
            .in_current_span(),
        );
        Self {
            _join_handle: join_handle,
        }
    }
}

pub(crate) async fn deliver_fetch(request: &EgressFetchRequest, sender: &dyn FetchSender) {
    let mut objects = FetchCursor::new(
        &request.cache,
        request.start_location,
        request.end_location,
        request.group_order,
    );
    loop {
        match objects.next().await {
            Ok(Some(object)) => {
                let object = match object.load().await {
                    Ok(object) => object,
                    Err(error) => {
                        tracing::warn!(
                            request_id = request.request_id,
                            ?error,
                            "cached object cannot be loaded; resetting fetch stream"
                        );
                        if let Err(e) = sender.reset(DataStreamResetCode::InternalError).await {
                            tracing::error!(error = ?e, "failed to reset fetch stream");
                        }
                        return;
                    }
                };
                if let Err(e) = sender.send(object).await {
                    tracing::error!(error = ?e, "failed to send fetch object");
                    return;
                }
            }
            Ok(None) => break,
            Err(interrupted) => {
                let error_code = match interrupted {
                    FetchInterrupted::Malformed => DataStreamResetCode::MalformedTrack,
                    FetchInterrupted::Incomplete => DataStreamResetCode::InternalError,
                };
                tracing::warn!(
                    request_id = request.request_id,
                    ?interrupted,
                    "fetch cannot be completed from cache; resetting fetch stream"
                );
                if let Err(e) = sender.reset(error_code).await {
                    tracing::error!(error = ?e, "failed to reset fetch stream");
                }
                return;
            }
        }
    }
    if request.cache.is_malformed() {
        if let Err(e) = sender.reset(DataStreamResetCode::MalformedTrack).await {
            tracing::error!(error = ?e, "failed to reset fetch stream");
        }
        return;
    }
    if let Err(e) = sender.close().await {
        tracing::error!(error = ?e, "failed to close fetch stream");
    }
}

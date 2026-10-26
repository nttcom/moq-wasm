use crate::modules::{
    data_plane::{
        cache::{
            cached_object::CachedObject,
            track_cache::{FetchCursor, FetchInterrupted, PlannedFetch},
        },
        egress::coordinator::EgressFetchRequest,
    },
    session::data_sender::fetch_sender::FetchSender,
};
use moqt::{fetch::UnknownRange, object::DataStreamResetCode};

pub(crate) async fn deliver_fetch(request: &EgressFetchRequest, sender: &dyn FetchSender) {
    if request.report_unknown {
        return deliver_reporting_unknown(request, sender).await;
    }
    let mut objects = FetchCursor::new(
        &request.cache,
        request.start_location,
        request.end_location,
        request.group_order,
    );
    loop {
        match objects.next().await {
            Ok(Some(object)) => {
                let Some(object) = load(request, sender, &object).await else {
                    return;
                };
                if let Err(e) = sender.send(object).await {
                    tracing::error!(?e, "failed to send fetch object");
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
                    tracing::error!(?e, "failed to reset fetch stream");
                }
                return;
            }
        }
    }
    finish(request, sender).await;
}

/// Sends what the cache holds now, each stretch it does not know reported
/// as unknown; a sender that cannot express that has reset the response.
async fn deliver_reporting_unknown(request: &EgressFetchRequest, sender: &dyn FetchSender) {
    let plan = request.cache.plan_fetch(
        request.start_location,
        request.end_location,
        request.group_order,
    );
    for step in plan {
        match step {
            PlannedFetch::Object(object) => {
                let Some(object) = load(request, sender, &object).await else {
                    return;
                };
                if let Err(e) = sender.send(object).await {
                    tracing::error!(?e, "failed to send fetch object");
                    return;
                }
            }
            PlannedFetch::Unknown { last } => match sender.report_unknown_range(last).await {
                Ok(UnknownRange::Marked) => {}
                Ok(_) => {
                    tracing::info!(
                        request_id = request.request_id,
                        ?last,
                        "fetch range has objects of unknown status; the fetch stream was reset"
                    );
                    return;
                }
                Err(e) => {
                    tracing::error!(?e, "failed to report an unknown fetch range");
                    return;
                }
            },
        }
    }
    finish(request, sender).await;
}

/// `None` once the response has been reset because the payload is lost.
async fn load(
    request: &EgressFetchRequest,
    sender: &dyn FetchSender,
    object: &CachedObject,
) -> Option<moqt::object::Object> {
    match object.load().await {
        Ok(object) => Some(object),
        Err(error) => {
            tracing::warn!(
                request_id = request.request_id,
                ?error,
                "cached object cannot be loaded; resetting fetch stream"
            );
            if let Err(e) = sender.reset(DataStreamResetCode::InternalError).await {
                tracing::error!(?e, "failed to reset fetch stream");
            }
            None
        }
    }
}

async fn finish(request: &EgressFetchRequest, sender: &dyn FetchSender) {
    if request.cache.is_malformed() {
        if let Err(e) = sender.reset(DataStreamResetCode::MalformedTrack).await {
            tracing::error!(?e, "failed to reset fetch stream");
        }
        return;
    }
    if let Err(e) = sender.close().await {
        tracing::error!(?e, "failed to close fetch stream");
    }
}

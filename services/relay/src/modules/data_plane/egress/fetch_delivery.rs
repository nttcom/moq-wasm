use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::modules::{
    data_plane::cache::{
        cached_object::CachedObject,
        track_cache::{FetchCursor, FetchInterrupted, PlannedFetch, TrackCache},
    },
    domain::session_id::SessionId,
    session::{data_sender::fetch_sender::FetchSender, publisher::Publisher},
};
use moqt::{fetch::UnknownRange, object::DataStreamResetCode};

pub(crate) struct EgressFetchRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) start_location: moqt::object::Location,
    pub(crate) end_location: moqt::object::Location,
    pub(crate) group_order: moqt::object::GroupOrder,
    pub(crate) wait: FetchWait,
}

/// How long a FETCH's delivery waits for objects of its range the cache
/// does not hold yet (draft-18 §10.2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchWait {
    /// Until they arrive.
    Unlimited,
    /// Until the instant, after which the rest is reported as unknown.
    Until(tokio::time::Instant),
    /// Not at all: what the cache holds now is served, each stretch it does
    /// not know reported as unknown.
    Never,
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
    let deadline = match request.wait {
        FetchWait::Never => return deliver_reporting_unknown(request, sender, None).await,
        FetchWait::Until(deadline) => Some(deadline),
        FetchWait::Unlimited => None,
    };
    let mut objects = FetchCursor::new(
        &request.cache,
        request.start_location,
        request.end_location,
        request.group_order,
    );
    let mut last_sent = None;
    loop {
        let next = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, objects.next()).await {
                Ok(next) => next,
                Err(_) => return deliver_reporting_unknown(request, sender, last_sent).await,
            },
            None => objects.next().await,
        };
        match next {
            Ok(Some(object)) => {
                let Some(object) = load(request, sender, &object).await else {
                    return;
                };
                let at = object.location;
                if let Err(e) = sender.send(object).await {
                    tracing::error!(error = ?e, "failed to send fetch object");
                    return;
                }
                last_sent = Some(at);
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
    finish(request, sender).await;
}

/// Sends what the cache holds now after `sent`, the last object already
/// sent, each stretch it does not know reported as unknown; a sender that
/// cannot express that has reset the response.
async fn deliver_reporting_unknown(
    request: &EgressFetchRequest,
    sender: &dyn FetchSender,
    sent: Option<moqt::object::Location>,
) {
    let plan = request.cache.plan_fetch(
        request.start_location,
        request.end_location,
        request.group_order,
    );
    let delivered = |at: moqt::object::Location| {
        sent.is_some_and(|sent| {
            delivery_order(request.group_order, at) <= delivery_order(request.group_order, sent)
        })
    };
    for step in plan.into_iter().skip_while(|step| match step {
        PlannedFetch::Object(object) => delivered(object.object.location),
        PlannedFetch::Unknown { last } => delivered(*last),
    }) {
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

/// A key that sorts locations as a FETCH in `group_order` delivers them:
/// groups in that order, the objects of a group ascending.
fn delivery_order(group_order: moqt::object::GroupOrder, at: moqt::object::Location) -> (u64, u64) {
    let group = match group_order {
        moqt::object::GroupOrder::Descending => u64::MAX - at.group_id,
        _ => at.group_id,
    };
    (group, at.object_id)
}

/// The object in the form a FETCH from the cache gives it, or `None` once
/// the response has been reset because the payload is lost.
async fn load(
    request: &EgressFetchRequest,
    sender: &dyn FetchSender,
    cached: &CachedObject,
) -> Option<moqt::object::Object> {
    match cached.load().await {
        Ok(mut object) => {
            object.forwarding = request
                .cache
                .fetch_forwarding(cached, request.subscriber_session_id);
            Some(object)
        }
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
            tracing::error!(error = ?e, "failed to reset fetch stream");
        }
        return;
    }
    if let Err(e) = sender.close().await {
        tracing::error!(error = ?e, "failed to close fetch stream");
    }
}

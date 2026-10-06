use crate::modules::{
    data_plane::{
        cache::track_cache::{FetchCursor, FetchInterrupted},
        egress::coordinator::EgressFetchRequest,
    },
    enums::FetchErrorCode,
    session::data_sender::fetch_sender::FetchSender,
};

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
                if let Err(e) = sender.send(object.to_fetch_object_field()).await {
                    tracing::error!(?e, "failed to send fetch object");
                    return;
                }
            }
            Ok(None) => break,
            Err(interrupted) => {
                let error_code = match interrupted {
                    FetchInterrupted::Malformed => FetchErrorCode::MalformedTrack as u64,
                    FetchInterrupted::Incomplete => FetchErrorCode::InternalError as u64,
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
    if request.cache.is_malformed() {
        if let Err(e) = sender.reset(FetchErrorCode::MalformedTrack as u64).await {
            tracing::error!(?e, "failed to reset fetch stream");
        }
        return;
    }
    if let Err(e) = sender.close().await {
        tracing::error!(?e, "failed to close fetch stream");
    }
}

use crate::modules::moqt::control_plane::{
    control_messages::messages::{
        fetch_ok::FetchOk, publish_ok::PublishOk, subscribe_ok::SubscribeOk,
    },
    error_codes::RequestErrorCode,
    handler::{
        fetch_cancel_handler::FetchCancelHandler, fetch_handler::FetchHandler,
        go_away_handler::GoAwayHandler, max_request_id_handler::MaxRequestIdHandler,
        publish_done_handler::PublishDoneHandler, publish_handler::PublishHandler,
        publish_namespace_cancel_handler::PublishNamespaceCancelHandler,
        publish_namespace_done_handler::PublishNamespaceDoneHandler,
        publish_namespace_handler::PublishNamespaceHandler,
        requests_blocked_handler::RequestsBlockedHandler, subscribe_handler::SubscribeHandler,
        subscribe_namespace_handler::SubscribeNamespaceHandler,
        subscribe_update_handler::SubscribeUpdateHandler, track_status_handler::TrackStatusHandler,
        unsubscribe_handler::UnsubscribeHandler,
        unsubscribe_namespace_handler::UnsubscribeNamespaceHandler,
    },
};

// message aliases
pub type RequestId = u64;

pub(crate) type ErrorPhrase = String;

#[derive(Clone, Debug)]
pub enum SessionEvent {
    GoAway(GoAwayHandler),
    MaxRequestId(MaxRequestIdHandler),
    RequestsBlocked(RequestsBlockedHandler),
    PublishNamespace(PublishNamespaceHandler),
    PublishNamespaceDone(PublishNamespaceDoneHandler),
    PublishNamespaceCancel(PublishNamespaceCancelHandler),
    SubscribeNameSpace(SubscribeNamespaceHandler),
    UnsubscribeNamespace(UnsubscribeNamespaceHandler),
    Publish(PublishHandler),
    PublishDone(PublishDoneHandler),
    Subscribe(SubscribeHandler),
    SubscribeUpdate(SubscribeUpdateHandler),
    Unsubscribe(UnsubscribeHandler),
    Fetch(FetchHandler),
    FetchCancel(FetchCancelHandler),
    TrackStatus(TrackStatusHandler),
    Disconnected(),
    ProtocolViolation(),
}

#[derive(Clone, Debug)]
pub(crate) enum ResponseMessage {
    SubscribeNameSpaceOk(RequestId),
    SubscribeNameSpaceError(RequestId, RequestErrorCode, ErrorPhrase),
    PublishNamespaceOk(RequestId),
    PublishNamespaceError(RequestId, RequestErrorCode, ErrorPhrase),
    PublishOk(PublishOk),
    PublishError(RequestId, RequestErrorCode, ErrorPhrase),
    SubscribeOk(SubscribeOk),
    SubscribeError(RequestId, RequestErrorCode, ErrorPhrase),
    FetchOk(FetchOk),
    FetchError(RequestId, RequestErrorCode, ErrorPhrase),
    TrackStatusOk(SubscribeOk),
    TrackStatusError(RequestId, RequestErrorCode, ErrorPhrase),
}

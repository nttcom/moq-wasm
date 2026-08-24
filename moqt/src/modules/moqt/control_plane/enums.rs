use crate::{
    TransportProtocol,
    modules::moqt::control_plane::{
        control_messages::messages::{
            fetch_ok::FetchOk, publish_ok::PublishOk, subscribe_ok::SubscribeOk,
        },
        handler::{
            fetch_cancel_handler::FetchCancelHandler, fetch_handler::FetchHandler,
            go_away_handler::GoAwayHandler, max_request_id_handler::MaxRequestIdHandler,
            publish_done_handler::PublishDoneHandler, publish_handler::PublishHandler,
            publish_namespace_cancel_handler::PublishNamespaceCancelHandler,
            publish_namespace_done_handler::PublishNamespaceDoneHandler,
            publish_namespace_handler::PublishNamespaceHandler,
            requests_blocked_handler::RequestsBlockedHandler, subscribe_handler::SubscribeHandler,
            subscribe_namespace_handler::SubscribeNamespaceHandler,
            subscribe_update_handler::SubscribeUpdateHandler,
            track_status_handler::TrackStatusHandler, unsubscribe_handler::UnsubscribeHandler,
            unsubscribe_namespace_handler::UnsubscribeNamespaceHandler,
        },
    },
};

// message aliases
pub type RequestId = u64;

pub(crate) type ErrorCode = u64;
pub(crate) type ErrorPhrase = String;

#[derive(Clone, Debug)]
pub enum SessionEvent<T: TransportProtocol> {
    GoAway(GoAwayHandler),
    MaxRequestId(MaxRequestIdHandler),
    RequestsBlocked(RequestsBlockedHandler),
    PublishNamespace(PublishNamespaceHandler<T>),
    PublishNamespaceDone(PublishNamespaceDoneHandler),
    PublishNamespaceCancel(PublishNamespaceCancelHandler),
    SubscribeNameSpace(SubscribeNamespaceHandler<T>),
    UnsubscribeNamespace(UnsubscribeNamespaceHandler),
    Publish(PublishHandler<T>),
    PublishDone(PublishDoneHandler),
    Subscribe(SubscribeHandler<T>),
    SubscribeUpdate(SubscribeUpdateHandler),
    Unsubscribe(UnsubscribeHandler<T>),
    Fetch(FetchHandler<T>),
    FetchCancel(FetchCancelHandler),
    TrackStatus(TrackStatusHandler<T>),
    Disconnected(),
    ProtocolViolation(),
}

#[derive(Clone, Debug)]
pub(crate) enum ResponseMessage {
    SubscribeNameSpaceOk(RequestId),
    SubscribeNameSpaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishNamespaceOk(RequestId),
    PublishNamespaceError(RequestId, ErrorCode, ErrorPhrase),
    PublishOk(PublishOk),
    PublishError(RequestId, ErrorCode, ErrorPhrase),
    SubscribeOk(SubscribeOk),
    SubscribeError(RequestId, ErrorCode, ErrorPhrase),
    FetchOk(FetchOk),
    FetchError(RequestId, ErrorCode, ErrorPhrase),
}

use crate::modules::core::handler::{
    fetch::FetchHandler, fetch_cancel::FetchCancelHandler, go_away::GoAwayHandler,
    max_request_id::MaxRequestIdHandler, publish::PublishHandler, publish_done::PublishDoneHandler,
    publish_namespace::PublishNamespaceHandler,
    publish_namespace_cancel::PublishNamespaceCancelHandler,
    publish_namespace_done::PublishNamespaceDoneHandler, requests_blocked::RequestsBlockedHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    subscribe_update::SubscribeUpdateHandler, track_status::TrackStatusHandler,
    unsubscribe::UnsubscribeHandler, unsubscribe_namespace::UnsubscribeNamespaceHandler,
};
use crate::modules::types::SessionId;

pub(crate) enum SessionEvent {
    GoAway(SessionId, Box<dyn GoAwayHandler>),
    MaxRequestId(SessionId, Box<dyn MaxRequestIdHandler>),
    RequestsBlocked(SessionId, Box<dyn RequestsBlockedHandler>),
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    PublishNamespaceDone(SessionId, Box<dyn PublishNamespaceDoneHandler>),
    PublishNamespaceCancel(SessionId, Box<dyn PublishNamespaceCancelHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    UnsubscribeNameSpace(SessionId, Box<dyn UnsubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    PublishDone(SessionId, Box<dyn PublishDoneHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    SubscribeUpdate(SessionId, Box<dyn SubscribeUpdateHandler>),
    Unsubscribe(SessionId, Box<dyn UnsubscribeHandler>),
    Fetch(SessionId, Box<dyn FetchHandler>),
    FetchCancel(SessionId, Box<dyn FetchCancelHandler>),
    TrackStatus(SessionId, Box<dyn TrackStatusHandler>),
    Disconnected(SessionId),
    ProtocolViolation(SessionId),
}

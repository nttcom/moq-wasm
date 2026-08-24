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

pub(crate) enum MoqtSessionEvent {
    GoAway(Box<dyn GoAwayHandler>),
    MaxRequestId(Box<dyn MaxRequestIdHandler>),
    RequestsBlocked(Box<dyn RequestsBlockedHandler>),
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    PublishNamespaceDone(Box<dyn PublishNamespaceDoneHandler>),
    PublishNamespaceCancel(Box<dyn PublishNamespaceCancelHandler>),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    UnsubscribeNamespace(Box<dyn UnsubscribeNamespaceHandler>),
    Publish(Box<dyn PublishHandler>),
    PublishDone(Box<dyn PublishDoneHandler>),
    Subscribe(Box<dyn SubscribeHandler>),
    SubscribeUpdate(Box<dyn SubscribeUpdateHandler>),
    Unsubscribe(Box<dyn UnsubscribeHandler>),
    Fetch(Box<dyn FetchHandler>),
    FetchCancel(Box<dyn FetchCancelHandler>),
    TrackStatus(Box<dyn TrackStatusHandler>),
    Disconnected(),
    ProtocolViolation(),
}

impl std::fmt::Debug for MoqtSessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MoqtSessionEvent::GoAway(_) => "GoAway",
            MoqtSessionEvent::MaxRequestId(_) => "MaxRequestId",
            MoqtSessionEvent::RequestsBlocked(_) => "RequestsBlocked",
            MoqtSessionEvent::PublishNamespace(_) => "PublishNamespace",
            MoqtSessionEvent::PublishNamespaceDone(_) => "PublishNamespaceDone",
            MoqtSessionEvent::PublishNamespaceCancel(_) => "PublishNamespaceCancel",
            MoqtSessionEvent::SubscribeNamespace(_) => "SubscribeNamespace",
            MoqtSessionEvent::UnsubscribeNamespace(_) => "UnsubscribeNamespace",
            MoqtSessionEvent::Publish(_) => "Publish",
            MoqtSessionEvent::PublishDone(_) => "PublishDone",
            MoqtSessionEvent::Subscribe(_) => "Subscribe",
            MoqtSessionEvent::SubscribeUpdate(_) => "SubscribeUpdate",
            MoqtSessionEvent::Unsubscribe(_) => "Unsubscribe",
            MoqtSessionEvent::Fetch(_) => "Fetch",
            MoqtSessionEvent::FetchCancel(_) => "FetchCancel",
            MoqtSessionEvent::TrackStatus(_) => "TrackStatus",
            MoqtSessionEvent::Disconnected() => "Disconnected",
            MoqtSessionEvent::ProtocolViolation() => "ProtocolViolation",
        };

        f.write_str(name)
    }
}

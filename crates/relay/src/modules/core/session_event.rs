use crate::modules::core::handler::{
    fetch::FetchHandler, publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
    subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    track_status::TrackStatusHandler, unsubscribe::UnsubscribeHandler,
};

pub(crate) enum MoqtSessionEvent {
    GoAway(moqt::GoAwayHandler),
    MaxRequestId(moqt::MaxRequestIdHandler),
    RequestsBlocked(moqt::RequestsBlockedHandler),
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    PublishNamespaceDone(moqt::PublishNamespaceDoneHandler),
    PublishNamespaceCancel(moqt::PublishNamespaceCancelHandler),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    UnsubscribeNamespace(moqt::UnsubscribeNamespaceHandler),
    Publish(Box<dyn PublishHandler>),
    PublishDone(moqt::PublishDoneHandler),
    Subscribe(Box<dyn SubscribeHandler>),
    SubscribeUpdate(moqt::SubscribeUpdateHandler),
    Unsubscribe(Box<dyn UnsubscribeHandler>),
    Fetch(Box<dyn FetchHandler>),
    FetchCancel(moqt::FetchCancelHandler),
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

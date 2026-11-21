use crate::modules::{
    domain::publish_done_reason::PublishDoneReason,
    session::handler::{
        fetch::FetchHandler, publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
        subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
        subscribe_namespace_update::SubscribeNamespaceUpdateHandler,
        subscribe_update::SubscribeUpdateHandler, track_status::TrackStatusHandler,
    },
};

pub(crate) enum MoqtSessionEvent {
    GoAway(moqt::GoAwayHandler),
    PublishNamespace(Box<dyn PublishNamespaceHandler>),
    PublishNamespaceDone(moqt::namespace::PublishNamespaceDoneHandler),
    PublishNamespaceCancel(moqt::namespace::PublishNamespaceCancelHandler),
    SubscribeNamespace(Box<dyn SubscribeNamespaceHandler>),
    SubscribeNamespaceUpdate(Box<dyn SubscribeNamespaceUpdateHandler>),
    UnsubscribeNamespace(moqt::namespace::UnsubscribeNamespaceHandler),
    PublishBlocked(moqt::namespace::PublishBlockedHandler),
    Publish(Box<dyn PublishHandler>),
    PublishDone {
        request_id: u64,
        reason: PublishDoneReason,
        stream_count: u64,
    },
    Subscribe(Box<dyn SubscribeHandler>),
    SubscribeUpdate(Box<dyn SubscribeUpdateHandler>),
    Unsubscribe {
        request_id: u64,
    },
    Fetch(Box<dyn FetchHandler>),
    FetchCancel(moqt::fetch::FetchCancelHandler),
    TrackStatus(Box<dyn TrackStatusHandler>),
    Disconnected(),
    ProtocolViolation(),
}

impl std::fmt::Debug for MoqtSessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MoqtSessionEvent::GoAway(_) => "GoAway",
            MoqtSessionEvent::PublishNamespace(_) => "PublishNamespace",
            MoqtSessionEvent::PublishNamespaceDone(_) => "PublishNamespaceDone",
            MoqtSessionEvent::PublishNamespaceCancel(_) => "PublishNamespaceCancel",
            MoqtSessionEvent::SubscribeNamespace(_) => "SubscribeNamespace",
            MoqtSessionEvent::SubscribeNamespaceUpdate(_) => "SubscribeNamespaceUpdate",
            MoqtSessionEvent::UnsubscribeNamespace(_) => "UnsubscribeNamespace",
            MoqtSessionEvent::PublishBlocked(_) => "PublishBlocked",
            MoqtSessionEvent::Publish(_) => "Publish",
            MoqtSessionEvent::PublishDone { .. } => "PublishDone",
            MoqtSessionEvent::Subscribe(_) => "Subscribe",
            MoqtSessionEvent::SubscribeUpdate(_) => "SubscribeUpdate",
            MoqtSessionEvent::Unsubscribe { .. } => "Unsubscribe",
            MoqtSessionEvent::Fetch(_) => "Fetch",
            MoqtSessionEvent::FetchCancel(_) => "FetchCancel",
            MoqtSessionEvent::TrackStatus(_) => "TrackStatus",
            MoqtSessionEvent::Disconnected() => "Disconnected",
            MoqtSessionEvent::ProtocolViolation() => "ProtocolViolation",
        };

        f.write_str(name)
    }
}

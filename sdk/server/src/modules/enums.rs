use uuid::Uuid;

// message alias
pub(crate) type RequestId = moqt::RequestId;
pub(crate) type TrackNamespaces = moqt::TrackNamespaces;
pub(crate) type GroupOrder = moqt::GroupOrder;
pub(crate) type IsContentExist = moqt::IsContentExist;
pub(crate) type IsForward = moqt::IsForward;
pub(crate) type SubscriberPriority = moqt::SubscriberPriority;
pub(crate) type FilterType = moqt::FilterType;

// parameter alias
pub(crate) type Authorization = moqt::Authorization;
pub(crate) type DeliveryTimeout = moqt::DeliveryTimeout;
pub(crate) type MaxCacheDuration = moqt::MaxCacheDuration;

pub(crate) enum SessionEvent {
    PublishNameSpace(Uuid, RequestId, TrackNamespaces, Vec<Authorization>),
    SubscribeNameSpace(Uuid, RequestId, TrackNamespaces, Vec<Authorization>),
    Publish(
        Uuid,
        RequestId,
        TrackNamespaces,
        GroupOrder,
        IsContentExist,
        IsForward,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
        Vec<MaxCacheDuration>,
    ),
    Subscribe(
        Uuid,
        RequestId,
        TrackNamespaces,
        SubscriberPriority,
        GroupOrder,
        IsContentExist,
        IsForward,
        FilterType,
        Vec<Authorization>,
        Vec<DeliveryTimeout>,
    ),
}

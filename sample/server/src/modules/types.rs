// moqt
pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;
pub(crate) type TrackName = String;

// id
pub(crate) type SessionId = uuid::Uuid;

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

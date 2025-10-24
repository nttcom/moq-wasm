// moqt
pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;
pub(crate) type TrackName = String;
pub(crate) type TrackNamespaceWithTrackName = String;

// id
pub(crate) type SessionId = uuid::Uuid;

// message alias
pub(crate) type TrackAlias = u64;
pub(crate) type GroupOrder = moqt::GroupOrder;
pub(crate) type IsContentExist = moqt::ContentExists;
pub(crate) type IsForward = moqt::Forward;
pub(crate) type SubscriberPriority = moqt::SubscriberPriority;

// parameter alias
pub(crate) type DeliveryTimeout = moqt::DeliveryTimeout;
pub(crate) type MaxCacheDuration = moqt::MaxCacheDuration;

//! One `Session` that subscribes, fetches, publishes tracks and watches
//! namespaces, and answers the peer's requests itself. `Session` owns the event loop of the
//! `moqt::Session` it wraps, so the lower-level `Session::receive_event` is
//! not for its caller. Namespaces are element lists.
//!
//! Requests the draft lets the peer reject fail with `RequestError`, which
//! carries the draft's error code; a FETCH fails with `FetchError`, which
//! wraps one.
pub use crate::modules::client::announcement::Announcement;
pub use crate::modules::client::event_handler::{EventHandler, HandlerId};
pub use crate::modules::client::fetch::Fetch;
pub use crate::modules::client::namespace_watch::NamespaceWatch;
pub use crate::modules::client::request_error::{FetchError, RequestError};
pub use crate::modules::client::session::{
    GroupStartOptions, PublishTrackOptions, Session, SubscribeOptions,
};
pub use crate::modules::client::subscription::{Group, PublishDone, ReceivedObject, Subscription};
pub use crate::modules::client::track_follower::{FollowEvent, TrackFollower};
pub use crate::modules::client::track_watch::{PublishRequest, TrackWatch, WatchEvent};
pub use crate::modules::client::track_writer::TrackWriter;
pub use crate::modules::moqt::domains::origin::PublishTrackError;

//! Broadcasts: a namespace with its tracks, MSF event timelines and media
//! sources of encoded frames, published and followed as the browser SDK's
//! `moq-wasm/media` and the Python SDK's `moq_wasm.media` do.

mod broadcast_end;
mod catalog;
mod event_timeline;
mod loc_timestamp;
mod local_broadcast;
mod media_source;
mod remote_broadcast;
mod watch_broadcasts;

pub use broadcast::{MediaSourceError, TrackKind};
pub use catalog::{BitratePreference, Catalog, TrackQuery};
pub use event_timeline::EventTimeline;
pub use loc_timestamp::{loc_timestamp_headers, loc_timestamp_micros};
pub use local_broadcast::{BroadcastBuilder, LocalBroadcast, create_broadcast};
pub use media_source::{MediaSource, MediaSourceOptions, Rendition};
pub use mediapack::AudioLevel;
pub use moqt::publish::TrackDemand;
pub use msf::{
    CatalogParseError, KnownPackaging, KnownTrackRole, Packaging, Track as CatalogTrack, TrackRole,
};
pub use remote_broadcast::{
    Catalogs, EventTimelineOptions, FollowOptions, FollowedTrack, RemoteBroadcast,
    RemoteEventTimeline,
};
pub use watch_broadcasts::{WatchOptions, watch_broadcasts};

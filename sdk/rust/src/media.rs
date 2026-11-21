//! Broadcasts: a namespace with its tracks, MSF event timelines and media
//! sources of encoded frames, as the browser SDK's `moq-wasm/media` and the
//! Python SDK's `moq_wasm.media` publish them.

mod broadcast_end;
mod event_timeline;
mod local_broadcast;
mod media_source;

pub use broadcast::{MediaSourceError, TrackKind};
pub use event_timeline::EventTimeline;
pub use local_broadcast::{BroadcastBuilder, LocalBroadcast, create_broadcast};
pub use media_source::{MediaSource, MediaSourceOptions, Rendition};
pub use mediapack::AudioLevel;
pub use moqt::publish::TrackDemand;

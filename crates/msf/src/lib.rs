pub mod catalog;
pub mod media_timeline;
pub mod track;
pub mod types;
mod validation;

pub use catalog::{CATALOG_TRACK_NAME, Catalog, CatalogParseError, InitData};
pub use media_timeline::MediaTimelineRecord;
pub use track::{Dependency, Track, channel_config, channel_count};
pub use types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole};

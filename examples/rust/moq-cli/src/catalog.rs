use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use media_streaming_format::{
    Catalog, KnownPackaging, KnownTrackRole, Packaging, Track, TrackRole,
};

/// Track name carrying the catalog on every namespace.
pub const TRACK: &str = "catalog";

pub fn build_video_catalog(namespace: &str, media_track: &str, codec: &str) -> Catalog {
    let track = Track {
        namespace: Some(namespace.to_string()),
        name: media_track.to_string(),
        packaging: Packaging::Known(KnownPackaging::Loc),
        role: Some(TrackRole::Known(KnownTrackRole::Video)),
        is_live: true,
        codec: Some(codec.to_string()),
        event_type: None,
        target_latency: None,
        label: None,
        render_group: None,
        alt_group: None,
        init_data: None,
        depends: None,
        temporal_id: None,
        spatial_id: None,
        mime_type: None,
        framerate: None,
        timescale: None,
        bitrate: None,
        width: None,
        height: None,
        sample_rate: None,
        channel_config: None,
        display_width: None,
        display_height: None,
        lang: None,
        parent_name: None,
        track_duration: None,
    };
    Catalog {
        version: Some(1),
        generated_at: Some(now_millis()),
        is_complete: Some(true),
        tracks: Some(vec![track]),
        delta_update: None,
        add_tracks: None,
        remove_tracks: None,
        clone_tracks: None,
    }
}

pub fn serialize(catalog: &Catalog) -> Result<Vec<u8>> {
    serde_json::to_vec(catalog).context("serialize catalog json")
}

pub fn parse(bytes: &[u8]) -> Result<Catalog> {
    serde_json::from_slice(bytes).context("parse catalog json")
}

pub fn packaging_str(packaging: &Packaging) -> String {
    match packaging {
        Packaging::Known(KnownPackaging::Loc) => "loc".to_string(),
        Packaging::Known(KnownPackaging::MediaTimeline) => "media-timeline".to_string(),
        Packaging::Known(KnownPackaging::EventTimeline) => "event-timeline".to_string(),
        Packaging::Other(s) => s.clone(),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_video_catalog() {
        let catalog = build_video_catalog("tokyo/cam01", "video", "avc3.640028");
        let bytes = serialize(&catalog).unwrap();
        let parsed = parse(&bytes).unwrap();

        let track = &parsed.tracks.unwrap()[0];
        assert_eq!(track.name, "video");
        assert_eq!(track.codec.as_deref(), Some("avc3.640028"));
        assert_eq!(packaging_str(&track.packaging), "loc");
    }
}

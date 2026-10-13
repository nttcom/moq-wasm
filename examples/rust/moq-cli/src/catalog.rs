use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use msf::{Catalog, KnownPackaging, KnownTrackRole, Packaging, Track};

pub fn build_video_catalog(namespace: &str, media_track: &str, codec: &str) -> Catalog {
    let mut track = Track::new(media_track, KnownPackaging::Loc, true);
    track.namespace = Some(namespace.to_string());
    track.role = Some(KnownTrackRole::Video.into());
    track.codec = Some(codec.to_string());
    Catalog::new(vec![track], now_millis())
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
        Packaging::Known(KnownPackaging::Cmaf) => "cmaf".to_string(),
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

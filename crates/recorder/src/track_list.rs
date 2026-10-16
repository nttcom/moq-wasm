use msf::{Catalog, Track, TrackRole};

use crate::track_selection::loc_tracks;

const HEADER: [&str; 6] = ["NAME", "ROLE", "CODEC", "SIZE", "BITRATE", "SAMPLE RATE"];

pub(crate) fn track_table(catalog: &Catalog) -> String {
    let rows: Vec<[String; 6]> = std::iter::once(HEADER.map(String::from))
        .chain(loc_tracks(catalog).map(row))
        .collect();
    let widths: [usize; 6] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or(0)
    });
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect();
    lines.join("\n")
}

fn row(track: &Track) -> [String; 6] {
    let size = track
        .width
        .zip(track.height)
        .map(|(width, height)| format!("{width}x{height}"));
    [
        track.name.clone(),
        or_dash(track.role.as_ref().and_then(role_label)),
        or_dash(track.codec.as_ref()),
        or_dash(size),
        or_dash(track.bitrate),
        or_dash(track.sample_rate),
    ]
}

fn role_label(role: &TrackRole) -> Option<String> {
    match serde_json::to_value(role).ok()? {
        serde_json::Value::String(label) => Some(label),
        _ => None,
    }
}

fn or_dash(value: Option<impl ToString>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use msf::{KnownPackaging, KnownTrackRole};

    use super::*;

    #[test]
    fn lists_the_loc_tracks_with_their_role_codec_and_quality() {
        // Arrange
        let mut camera = Track::new("camera_main_720p", KnownPackaging::Loc, true);
        camera.role = Some(KnownTrackRole::Video.into());
        camera.codec = Some("avc1.4D4028".to_string());
        camera.width = Some(1280);
        camera.height = Some(720);
        camera.bitrate = Some(500_000);
        let mut audio = Track::new("audio_128kbps", KnownPackaging::Loc, true);
        audio.role = Some(KnownTrackRole::Audio.into());
        audio.codec = Some("opus".to_string());
        audio.bitrate = Some(128_000);
        audio.sample_rate = Some(48_000);
        let cmaf = camera.cmaf_sibling("camera_main_720p_cmaf", "video/mp4", None);
        let mut chat = Track::new("chat", KnownPackaging::EventTimeline, true);
        chat.role = Some(TrackRole::Other("chat".to_string()));
        let catalog = Catalog::new(vec![camera, cmaf, audio, chat], 0);

        // Act
        let table = track_table(&catalog);

        // Assert
        assert_eq!(
            table,
            "NAME              ROLE   CODEC        SIZE      BITRATE  SAMPLE RATE\n\
             camera_main_720p  video  avc1.4D4028  1280x720  500000   -\n\
             audio_128kbps     audio  opus         -         128000   48000"
        );
    }
}

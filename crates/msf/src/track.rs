use serde::{Deserialize, Serialize};

use crate::catalog::InitData;
use crate::types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole};

/// An element of `depends` (draft-ietf-moq-msf-02 §5.2.15): a String names
/// a track, a Number a `renderGroup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    TrackName(String),
    RenderGroup(u64),
}

impl Dependency {
    /// `depends` naming the tracks `names` among `tracks`: their
    /// `renderGroup` when they are exactly its tracks, which
    /// draft-ietf-moq-msf-02 §7.2 requires of a media timeline, and their
    /// names otherwise.
    pub fn on_tracks(tracks: &[Track], names: Vec<String>) -> Vec<Self> {
        let render_group = |name: &String| {
            tracks
                .iter()
                .find(|track| &track.name == name)
                .and_then(|track| track.render_group)
        };
        let mut render_groups = names.iter().map(render_group);
        if let Some(Some(group)) = render_groups.next()
            && render_groups.all(|other| other == Some(group))
            && tracks
                .iter()
                .filter(|track| track.render_group == Some(group))
                .count()
                == names.len()
        {
            return vec![Self::RenderGroup(group)];
        }
        names.into_iter().map(Self::TrackName).collect()
    }
}

impl From<&str> for Dependency {
    fn from(track_name: &str) -> Self {
        Self::TrackName(track_name.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub name: String,
    pub packaging: Packaging,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<TrackRole>,
    pub is_live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_latency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<Dependency>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spatial_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub framerate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timescale: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(rename = "samplerate", skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_grp_sap_starting_type: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_obj_sap_starting_type: Option<u32>,
}

impl Track {
    pub fn has_role(&self, role: KnownTrackRole) -> bool {
        self.role == Some(TrackRole::Known(role))
    }

    /// The tracks of `tracks` this track's `depends` refers to
    /// (draft-ietf-moq-msf-02 §5.2.15): the one each name names and every
    /// other track of each `renderGroup`.
    pub fn dependencies<'a>(&'a self, tracks: &'a [Track]) -> impl Iterator<Item = &'a Track> {
        self.depends.iter().flatten().flat_map(move |dependency| {
            tracks.iter().filter(move |other| {
                other.name != self.name
                    && match dependency {
                        Dependency::TrackName(name) => &other.name == name,
                        Dependency::RenderGroup(group) => other.render_group == Some(*group),
                    }
            })
        })
    }

    /// The fields draft-ietf-moq-msf-02 §5.2 requires of every track object.
    pub fn new(name: &str, packaging: impl Into<Packaging>, is_live: bool) -> Self {
        Self {
            namespace: None,
            name: name.to_string(),
            packaging: packaging.into(),
            event_type: None,
            role: None,
            is_live,
            target_latency: None,
            label: None,
            render_group: None,
            alt_group: None,
            init_ref: None,
            depends: None,
            temporal_id: None,
            spatial_id: None,
            codec: None,
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
            max_grp_sap_starting_type: None,
            max_obj_sap_starting_type: None,
        }
    }

    /// Every group and object starts on a SAP type 1 sample
    /// (draft-ietf-moq-cmsf-01 §3.5.2) and `altGroup` names the switching set
    /// (§3.2). The sibling does not inherit `initRef`; set its CMAF header
    /// with `set_inline_init_data` (§3.1).
    pub fn cmaf_sibling(&self, name: &str, mime_type: &str, alt_group: Option<u64>) -> Self {
        Self {
            name: name.to_string(),
            packaging: Packaging::Known(KnownPackaging::Cmaf),
            alt_group,
            init_ref: None,
            mime_type: Some(mime_type.to_string()),
            max_grp_sap_starting_type: Some(1),
            max_obj_sap_starting_type: Some(1),
            ..self.clone()
        }
    }

    /// Points `initRef` at the returned inline `initDataList` entry; `data`
    /// must already be Base64 (draft-ietf-moq-cmsf-01 §3.1). The entry id is
    /// the track name, so ids stay unique as long as track names do.
    #[must_use = "add the entry to the catalog initDataList"]
    pub fn set_inline_init_data(&mut self, data: String) -> InitData {
        self.init_ref = Some(self.name.clone());
        InitData {
            id: self.name.clone(),
            r#type: "inline".to_string(),
            data,
        }
    }
}

/// The number of channels a `channelConfig` (draft-ietf-moq-msf-02 §5.2.30)
/// names, as `channel_config` writes it (`mono`, `stereo`, `<n>ch`) or as the
/// draft's examples do (`<n>`). `None` for any other label or zero channels.
pub fn channel_count(channel_config: &str) -> Option<u8> {
    let count = match channel_config {
        "mono" => 1,
        "stereo" => 2,
        other => other.trim_suffix("ch").parse().ok()?,
    };
    (count > 0).then_some(count)
}

/// The `channelConfig` (draft-ietf-moq-msf-02 §5.2.30) of `channels` channels.
pub fn channel_config(channels: u8) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KnownTrackRole;

    fn in_render_group(name: &str, render_group: Option<u64>) -> Track {
        let mut track = Track::new(name, KnownPackaging::Loc, true);
        track.render_group = render_group;
        track
    }

    #[test]
    fn depends_on_every_track_of_a_render_group_names_the_render_group() {
        // Arrange
        let tracks = [
            in_render_group("video", Some(1)),
            in_render_group("audio", Some(1)),
        ];

        // Act
        let depends = Dependency::on_tracks(&tracks, vec!["video".into(), "audio".into()]);

        // Assert
        assert_eq!(depends, [Dependency::RenderGroup(1)]);
    }

    #[test]
    fn depends_on_the_only_track_of_a_render_group_names_the_render_group() {
        // Arrange
        let tracks = [
            in_render_group("screen", None),
            in_render_group("video", Some(1)),
        ];

        // Act
        let depends = Dependency::on_tracks(&tracks, vec!["video".into()]);

        // Assert
        assert_eq!(depends, [Dependency::RenderGroup(1)]);
    }

    #[test]
    fn depends_on_part_of_a_render_group_or_on_tracks_without_one_names_the_tracks() {
        // Arrange
        let tracks = [
            in_render_group("video", Some(1)),
            in_render_group("audio", Some(1)),
            in_render_group("screen", None),
        ];

        // Act
        let part = Dependency::on_tracks(&tracks, vec!["video".into()]);
        let ungrouped = Dependency::on_tracks(&tracks, vec!["screen".into()]);

        // Assert
        assert_eq!(part, [Dependency::from("video")]);
        assert_eq!(ungrouped, [Dependency::from("screen")]);
    }

    #[test]
    fn dependencies_resolve_track_names_and_render_groups() {
        // Arrange
        let mut timeline = Track::new("timeline", KnownPackaging::MediaTimeline, true);
        timeline.depends = Some(vec![Dependency::RenderGroup(1), "screen".into()]);
        let tracks = [
            in_render_group("video", Some(1)),
            in_render_group("audio", Some(1)),
            in_render_group("screen", None),
            timeline.clone(),
        ];

        // Act
        let names: Vec<&str> = timeline
            .dependencies(&tracks)
            .map(|track| track.name.as_str())
            .collect();

        // Assert
        assert_eq!(names, ["video", "audio", "screen"]);
    }

    #[test]
    fn has_role_matches_only_the_track_role() {
        // Arrange
        let mut video = Track::new("video", KnownPackaging::Loc, true);
        video.role = Some(KnownTrackRole::Video.into());
        let unknown = Track::new("data", KnownPackaging::Loc, true);

        // Act / Assert
        assert!(video.has_role(KnownTrackRole::Video));
        assert!(!video.has_role(KnownTrackRole::Audio));
        assert!(!unknown.has_role(KnownTrackRole::Video));
    }

    #[test]
    fn new_carries_only_the_required_fields() {
        // Act
        let track = Track::new("video", KnownPackaging::Loc, true);

        // Assert
        assert_eq!(
            serde_json::to_string(&track).unwrap(),
            r#"{"name":"video","packaging":"loc","isLive":true}"#
        );
    }

    #[test]
    fn cmaf_sibling_shares_the_properties_and_adds_the_cmsf_fields() {
        // Arrange
        let mut loc = Track::new("video", KnownPackaging::Loc, true);
        loc.namespace = Some("live/test".to_string());
        loc.role = Some(KnownTrackRole::Video.into());
        loc.codec = Some("avc1.640028".to_string());
        loc.width = Some(1280);
        loc.init_ref = Some("video".to_string());

        // Act
        let cmaf = loc.cmaf_sibling("video_cmaf", "video/mp4", Some(2));

        // Assert
        assert_eq!(cmaf.name, "video_cmaf");
        assert_eq!(cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(cmaf.mime_type.as_deref(), Some("video/mp4"));
        assert_eq!(cmaf.init_ref, None);
        assert_eq!(cmaf.alt_group, Some(2));
        assert_eq!(cmaf.max_grp_sap_starting_type, Some(1));
        assert_eq!(cmaf.max_obj_sap_starting_type, Some(1));
        assert_eq!(cmaf.namespace, loc.namespace);
        assert_eq!(cmaf.role, loc.role);
        assert_eq!(cmaf.codec, loc.codec);
        assert_eq!(cmaf.width, loc.width);
    }

    #[test]
    fn set_inline_init_data_points_init_ref_at_the_entry_it_returns() {
        // Arrange
        let mut track = Track::new("audio_cmaf", KnownPackaging::Cmaf, true);

        // Act
        let init_data = track.set_inline_init_data("AAAA".to_string());

        // Assert
        assert_eq!(track.init_ref.as_deref(), Some("audio_cmaf"));
        assert_eq!(
            init_data,
            InitData {
                id: "audio_cmaf".to_string(),
                r#type: "inline".to_string(),
                data: "AAAA".to_string(),
            }
        );
    }

    #[test]
    fn sap_starting_types_use_the_cmsf_field_names() {
        // Arrange
        let mut track = Track::new("video", KnownPackaging::Cmaf, true);
        track.max_grp_sap_starting_type = Some(1);
        track.max_obj_sap_starting_type = Some(2);

        // Act
        let json = serde_json::to_string(&track).unwrap();

        // Assert
        assert!(json.contains("\"maxGrpSapStartingType\":1"));
        assert!(json.contains("\"maxObjSapStartingType\":2"));
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), track);
    }

    #[test]
    fn channel_config_round_trips_through_channel_count() {
        // Act / Assert
        for channels in [1, 2, 6] {
            assert_eq!(channel_count(&channel_config(channels)), Some(channels));
        }
    }

    #[test]
    fn channel_count_reads_the_draft_notation_and_rejects_no_channel() {
        // Act / Assert
        assert_eq!(channel_count("2"), Some(2));
        assert_eq!(channel_count("0"), None);
        assert_eq!(channel_count("5.1"), None);
    }
}

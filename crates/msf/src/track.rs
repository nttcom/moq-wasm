use serde::{Deserialize, Serialize};

use crate::types::{KnownPackaging, Packaging, TrackRole};

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
    pub init_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
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
    /// The fields draft-ietf-moq-msf-01 §5.2 requires of every track object.
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
            init_data: None,
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
    /// (§3.2).
    pub fn cmaf_sibling(
        &self,
        name: &str,
        mime_type: &str,
        init_data: String,
        alt_group: Option<u64>,
    ) -> Self {
        Self {
            name: name.to_string(),
            packaging: Packaging::Known(KnownPackaging::Cmaf),
            alt_group,
            mime_type: Some(mime_type.to_string()),
            init_data: Some(init_data),
            max_grp_sap_starting_type: Some(1),
            max_obj_sap_starting_type: Some(1),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KnownTrackRole;

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

        // Act
        let cmaf = loc.cmaf_sibling("video_cmaf", "video/mp4", "AAAA".to_string(), Some(2));

        // Assert
        assert_eq!(cmaf.name, "video_cmaf");
        assert_eq!(cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(cmaf.mime_type.as_deref(), Some("video/mp4"));
        assert_eq!(cmaf.init_data.as_deref(), Some("AAAA"));
        assert_eq!(cmaf.alt_group, Some(2));
        assert_eq!(cmaf.max_grp_sap_starting_type, Some(1));
        assert_eq!(cmaf.max_obj_sap_starting_type, Some(1));
        assert_eq!(cmaf.namespace, loc.namespace);
        assert_eq!(cmaf.role, loc.role);
        assert_eq!(cmaf.codec, loc.codec);
        assert_eq!(cmaf.width, loc.width);
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
}

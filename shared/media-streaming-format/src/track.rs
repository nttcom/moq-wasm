use serde::{Deserialize, Serialize};

use crate::types::{Packaging, TrackRole};

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
    use crate::types::{KnownPackaging, Packaging};

    #[test]
    fn sap_starting_types_use_the_cmsf_field_names() {
        // Arrange
        let track = Track {
            namespace: None,
            name: "video".into(),
            packaging: Packaging::Known(KnownPackaging::Cmaf),
            event_type: None,
            role: None,
            is_live: true,
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
            max_grp_sap_starting_type: Some(1),
            max_obj_sap_starting_type: Some(2),
        };

        // Act
        let json = serde_json::to_string(&track).unwrap();

        // Assert
        assert!(json.contains("\"maxGrpSapStartingType\":1"));
        assert!(json.contains("\"maxObjSapStartingType\":2"));
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), track);
    }
}

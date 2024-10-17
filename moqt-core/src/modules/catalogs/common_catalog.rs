use crate::catalogs::patch::JsonPatchOp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CommonCatalogFormat {
    CommonCatalog(CommonCatalog),
    CommonCatalogWithMultipleCatalogs(CommonCatalogWithMultipleCatalogs),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommonCatalog {
    version: u64,
    streaming_format: String,
    streaming_format_version: String,
    supports_delta_updates: Option<bool>,
    tracks: Option<Vec<Track>>,
    common_track_fields: Option<CommonTrackFields>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommonCatalogWithMultipleCatalogs {
    version: u64,
    catalogs: Option<Vec<CatalogObject>>,
    common_track_fields: Option<CommonTrackFields>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CatalogObject {
    namespace: Option<String>,
    name: String,
    streaming_format: String,
    streaming_format_version: String,
    supports_delta_updates: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
// inherit to each Track
pub struct CommonTrackFields {
    namespace: Option<String>,
    name: Option<String>,
    packaging: Option<String>,
    operation: Option<String>,
    label: Option<String>,
    render_group: Option<u64>,
    alt_group: Option<u64>,
    init_data: Option<String>,
    init_track: Option<String>,
    selection_params: Option<SelectionParams>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    namespace: Option<String>,
    name: Option<String>,
    packaging: Option<String>,
    operation: Option<String>,
    label: Option<String>,
    render_group: Option<u64>,
    alt_group: Option<u64>,
    init_data: Option<String>,
    init_track: Option<String>,
    selection_params: Option<SelectionParams>,
    depends: Option<Vec<String>>,
    temporal_id: Option<u64>,
    spatial_id: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SelectionParams {
    codec: Option<String>,
    mimetype: Option<String>,
    framerate: Option<u64>,
    bitrate: Option<u64>,
    width: Option<u64>,
    height: Option<u64>,
    samplerate: Option<u64>,
    channel_config: Option<String>,
    display_width: Option<u64>,
    display_height: Option<u64>,
    language: Option<String>,
}

#[cfg(test)]
mod serialize_success {
    use super::*;

    #[test]
    fn example1() {
        let json = r#"{
          "version": 1,
          "streamingFormat": "1",
          "streamingFormatVersion": "0.2",
          "commonTrackFields": {
            "namespace": "conference.example.com/conference123/alice",
            "packaging": "loc",
            "renderGroup": 1
          },
          "tracks": [
            {
              "name": "video",
              "selectionParams":{"codec":"av01.0.08M.10.0.110.09","width":1920,"height":1080,"framerate":30,"bitrate":1500000}
            },
            {
              "name": "audio",
              "selectionParams":{"codec":"opus","samplerate":48000,"channelConfig":"2","bitrate":32000}
            }
          ]
        }"#;
        let result: Result<CommonCatalogFormat, _> = serde_json::from_str(json);
        let expected = CommonCatalog {
            version: 1,
            streaming_format: "1".to_string(),
            streaming_format_version: "0.2".to_string(),
            common_track_fields: Some(CommonTrackFields {
                namespace: Some("conference.example.com/conference123/alice".to_string()),
                packaging: Some("loc".to_string()),
                render_group: Some(1),
                ..Default::default()
            }),
            tracks: Some(vec![
                Track {
                    name: Some("video".to_string()),
                    selection_params: Some(SelectionParams {
                        codec: Some("av01.0.08M.10.0.110.09".to_string()),
                        width: Some(1920),
                        height: Some(1080),
                        framerate: Some(30),
                        bitrate: Some(1500000),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Track {
                    name: Some("audio".to_string()),
                    selection_params: Some(SelectionParams {
                        codec: Some("opus".to_string()),
                        samplerate: Some(48000),
                        channel_config: Some("2".to_string()),
                        bitrate: Some(32000),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        match result.unwrap() {
            CommonCatalogFormat::CommonCatalog(result) => {
                assert_eq!(result, expected);
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn example11() {
        let json = r#"
        {
          "version": 1,
          "catalogs": [
            {
              "name": "catalog-for-format-one",
              "namespace": "sports.example.com/games/08-08-23/live",
              "streamingFormat": "1",
              "streamingFormatVersion": "0.2",
              "supportsDeltaUpdates": true
            },
            {
              "name": "catalog-for-format-five",
              "namespace": "chat.example.com/games/08-08-23/chat",
              "streamingFormat": "5",
              "streamingFormatVersion": "1.6.2"
            }
          ]
        }
        "#;
        let result: Result<CommonCatalogFormat, _> = serde_json::from_str(json);

        let expected = CommonCatalogWithMultipleCatalogs {
            version: 1,
            catalogs: Some(vec![
                CatalogObject {
                    name: "catalog-for-format-one".to_string(),
                    namespace: Some("sports.example.com/games/08-08-23/live".to_string()),
                    streaming_format: "1".to_string(),
                    streaming_format_version: "0.2".to_string(),
                    supports_delta_updates: Some(true),
                },
                CatalogObject {
                    name: "catalog-for-format-five".to_string(),
                    namespace: Some("chat.example.com/games/08-08-23/chat".to_string()),
                    streaming_format: "5".to_string(),
                    streaming_format_version: "1.6.2".to_string(),
                    supports_delta_updates: None,
                },
            ]),
            ..Default::default()
        };

        match result.unwrap() {
            CommonCatalogFormat::CommonCatalogWithMultipleCatalogs(result) => {
                assert_eq!(result, expected);
            }
            _ => panic!("unexpected"),
        }
    }
}

#[cfg(test)]
mod patch_success {

    use super::*;

    #[test]
    fn example1() {
        let json = r#"{
          "version": 1,
          "streamingFormat": "1",
          "streamingFormatVersion": "0.2",
          "commonTrackFields": {
            "namespace": "conference.example.com/conference123/alice",
            "packaging": "loc",
            "renderGroup": 1
          },
          "tracks": [
            {
              "name": "video",
              "selectionParams":{"codec":"av01.0.08M.10.0.110.09","width":1920,"height":1080,"framerate":30,"bitrate":1500000}
            },
            {
              "name": "audio",
              "selectionParams":{"codec":"opus","samplerate":48000,"channelConfig":"2","bitrate":32000}
            }
          ]
        }"#;
        let base_common_catalog: Result<CommonCatalogFormat, _> = serde_json::from_str(json);
        let base_common_catalog = match base_common_catalog {
            Ok(CommonCatalogFormat::CommonCatalog(common_catalog)) => common_catalog,
            _ => panic!("unexpected"),
        };

        let json = r#"{"op":"add","path":"tracks","value":"bar2"}"#;
        let result: JsonPatchOp = serde_json::from_str(json).unwrap();
        base_common_catalog.patch(vec![result]);
    }
}

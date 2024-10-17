use anyhow::Result;
use serde::{Deserialize, Serialize};
#[macro_use]
use json_patch;
use serde_json::{from_value, json};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CommonCatalogFormat {
    CommonCatalog(CommonCatalog),
    CommonCatalogWithMultipleCatalogs(CommonCatalogWithMultipleCatalogs),
}

pub trait JsonPatch {
    fn patch(&self, patch: json_patch::Patch) -> Result<Self>
    where
        Self: Sized + serde::Serialize + serde::de::DeserializeOwned,
    {
        let mut json = serde_json::to_value(self).unwrap();
        json_patch::patch(&mut json, &patch)?;
        let result: Self = serde_json::from_value(json).unwrap();
        Ok(result)
    }

    fn to_json(&self) -> serde_json::Value
    where
        Self: serde::Serialize,
    {
        serde_json::to_value(self).unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommonCatalog {
    version: u64,
    streaming_format: String,
    streaming_format_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_delta_updates: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracks: Option<Vec<Track>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    common_track_fields: Option<CommonTrackFields>,
}

impl JsonPatch for CommonCatalog {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommonCatalogWithMultipleCatalogs {
    version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalogs: Option<Vec<CatalogObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    common_track_fields: Option<CommonTrackFields>,
}

impl JsonPatch for CommonCatalogWithMultipleCatalogs {}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CatalogObject {
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    name: String,
    streaming_format: String,
    streaming_format_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_delta_updates: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
// inherit to each Track
pub struct CommonTrackFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packaging: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    render_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    init_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    init_track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_params: Option<SelectionParams>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packaging: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    render_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    init_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    init_track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_params: Option<SelectionParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    depends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temporal_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spatial_id: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SelectionParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mimetype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    framerate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    samplerate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
        let base_common_catalog_result: Result<CommonCatalogFormat, _> = serde_json::from_str(json);
        let mut base_common_catalog = match base_common_catalog_result {
            Ok(CommonCatalogFormat::CommonCatalog(common_catalog)) => common_catalog,
            _ => panic!("unexpected"),
        };

        let p: json_patch::Patch = from_value(json!([
          {
            "op": "add",
            "path": "/tracks/-",
            "value": {
              "name": "screen",
              "selectionParams":
                {
                  "codec":"vp8",
                  "width": 1920,
                  "height": 1080,
                  "framerate": 30,
                  "bitrate": 1500000
                }
              }
          }
        ]))
        .unwrap();
        base_common_catalog = base_common_catalog.patch(p).unwrap();
        assert_eq!(
            base_common_catalog.to_json(),
            json!({
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
                },
                {
                  "name": "screen",
                  "selectionParams":{"codec":"vp8","width": 1920,"height": 1080,"framerate": 30,"bitrate": 1500000}
                }
              ]
            })
        );
    }
}

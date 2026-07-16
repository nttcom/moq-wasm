use moqt::ExtensionHeaders;
use serde::{Deserialize, Serialize};

pub mod header_extension;
pub use header_extension::{
    AudioLevel, CaptureTimestamp, LOC_AUDIO_LEVEL_ID, LOC_CAPTURE_TIMESTAMP_ID,
    LOC_VIDEO_CONFIG_ID, LOC_VIDEO_FRAME_MARKING_ID, LocHeaderExtension, LocHeaderValue,
    UnknownHeaderExtension, VideoConfig, VideoFrameMarking,
};
#[cfg(feature = "wasm")]
pub mod wasm;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocHeader {
    pub extensions: Vec<LocHeaderExtension>,
}

impl LocHeader {
    pub fn to_extension_headers(&self) -> ExtensionHeaders {
        ExtensionHeaders::new(
            self.extensions
                .iter()
                .map(LocHeaderExtension::to_key_value_pair)
                .collect(),
        )
    }

    pub fn from_extension_headers(headers: &ExtensionHeaders) -> Self {
        Self {
            extensions: headers
                .key_value_pairs
                .iter()
                .map(LocHeaderExtension::from_key_value_pair)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocObject {
    pub header: LocHeader,

    pub payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use moqt::VariantType;

    #[test]
    fn even_odd_parity_matches_loc_spec() {
        // Even ids carry a varint value; the odd id (VideoConfig) carries bytes.
        let capture = LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
            micros_since_unix_epoch: 5,
        })
        .to_key_value_pair();
        assert_eq!(capture.key, LOC_CAPTURE_TIMESTAMP_ID);
        assert!(matches!(capture.value, VariantType::Even(5)));

        let config = LocHeaderExtension::VideoConfig(VideoConfig {
            data: vec![1, 2, 3],
        })
        .to_key_value_pair();
        assert_eq!(config.key, LOC_VIDEO_CONFIG_ID);
        assert!(matches!(config.value, VariantType::Odd(_)));
    }

    #[test]
    fn known_extensions_round_trip() {
        let extensions = vec![
            LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
                micros_since_unix_epoch: 123,
            }),
            LocHeaderExtension::VideoFrameMarking(VideoFrameMarking { flags: 0b101 }),
            LocHeaderExtension::AudioLevel(AudioLevel { level: 42 }),
            LocHeaderExtension::VideoConfig(VideoConfig {
                data: vec![9, 8, 7],
            }),
        ];
        for ext in extensions {
            assert_eq!(
                ext,
                LocHeaderExtension::from_key_value_pair(&ext.to_key_value_pair())
            );
        }
    }

    #[test]
    fn unknown_extension_round_trips_both_parities() {
        let even = LocHeaderExtension::Unknown(UnknownHeaderExtension {
            id: 8,
            value: LocHeaderValue::Even(77),
        });
        let odd = LocHeaderExtension::Unknown(UnknownHeaderExtension {
            id: 9,
            value: LocHeaderValue::Odd(vec![1, 2]),
        });
        assert_eq!(
            even,
            LocHeaderExtension::from_key_value_pair(&even.to_key_value_pair())
        );
        assert_eq!(
            odd,
            LocHeaderExtension::from_key_value_pair(&odd.to_key_value_pair())
        );
    }

    #[test]
    fn loc_header_preserves_order_and_unknown() {
        let header = LocHeader {
            extensions: vec![
                LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
                    micros_since_unix_epoch: 1,
                }),
                LocHeaderExtension::Unknown(UnknownHeaderExtension {
                    id: 21,
                    value: LocHeaderValue::Odd(vec![0xFF]),
                }),
                LocHeaderExtension::VideoConfig(VideoConfig { data: vec![5] }),
            ],
        };
        assert_eq!(
            header,
            LocHeader::from_extension_headers(&header.to_extension_headers())
        );
    }
}

use base64::{Engine, engine::general_purpose};
use mediapack::aac::AudioSpecificConfig;

/// Payload layout shared with examples/browser/examples/media:
/// `[meta_len (u32 BE)][meta JSON][coded data]`.
pub fn pack_video_chunk_payload(
    is_key: bool,
    timestamp_us: u64,
    sent_at_ms: u64,
    data: &[u8],
    codec_info: Option<&str>,
    description_base64: Option<&str>,
) -> Vec<u8> {
    let meta = serde_json::json!({
        "type": if is_key { "key" } else { "delta" },
        "timestamp": timestamp_us as i64,
        "duration": 0i64,
        "sentAt": sent_at_ms as i64,
        "codec": codec_info,
        "descriptionBase64": description_base64,
    });
    pack(&meta, data)
}

pub fn pack_audio_chunk_payload(
    data: &[u8],
    config: &AudioSpecificConfig,
    timestamp_us: u64,
    duration_us: u64,
    sent_at_ms: u64,
) -> Vec<u8> {
    let meta = serde_json::json!({
        "type": "key",
        "timestamp": timestamp_us as i64,
        "duration": duration_us as i64,
        "sentAt": sent_at_ms as i64,
        "codec": config.codec_string(),
        "descriptionBase64": general_purpose::STANDARD.encode(config.to_bytes()),
        "sampleRate": config.sample_rate as i64,
        "channels": config.channel_count() as i64,
    });
    pack(&meta, data)
}

fn pack(meta: &serde_json::Value, data: &[u8]) -> Vec<u8> {
    let meta_bytes = meta.to_string().into_bytes();
    let mut out = Vec::with_capacity(4 + meta_bytes.len() + data.len());
    out.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&meta_bytes);
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(payload: &[u8]) -> (serde_json::Value, &[u8]) {
        let meta_len = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
        (
            serde_json::from_slice(&payload[4..4 + meta_len]).unwrap(),
            &payload[4 + meta_len..],
        )
    }

    #[test]
    fn video_payload_carries_codec_only_when_given() {
        // Arrange
        let data = [0, 0, 0, 1, 0x65];

        // Act
        let payload =
            pack_video_chunk_payload(true, 1_000, 2_000, &data, Some("avc1.42C01E"), None);
        let (meta, coded) = split(&payload);

        // Assert
        assert_eq!(meta["type"], "key");
        assert_eq!(meta["timestamp"], 1_000);
        assert_eq!(meta["codec"], "avc1.42C01E");
        assert!(meta["descriptionBase64"].is_null());
        assert_eq!(coded, data);
    }

    #[test]
    fn audio_payload_describes_config() {
        // Arrange
        let config = AudioSpecificConfig::new(2, 48_000, 2);

        // Act
        let payload = pack_audio_chunk_payload(&[7], &config, 5, 21_333, 9);
        let (meta, coded) = split(&payload);

        // Assert
        assert_eq!(meta["codec"], "mp4a.40.2");
        assert_eq!(meta["descriptionBase64"], "EZA=");
        assert_eq!(meta["sampleRate"], 48_000);
        assert_eq!(meta["channels"], 2);
        assert_eq!(meta["duration"], 21_333);
        assert_eq!(coded, [7]);
    }
}

use anyhow::Result;
use base64::{Engine, engine::general_purpose};

const AAC_SAMPLES_PER_FRAME: u64 = 1024;

#[derive(Default)]
pub struct AacState {
    audio_specific_config: Option<Vec<u8>>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
    config_sent: bool,
}

pub struct AudioFrame {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u8,
    pub include_config: bool,
    pub audio_specific_config: Vec<u8>,
}

pub fn compute_aac_duration_us(sample_rate: u32) -> u64 {
    (AAC_SAMPLES_PER_FRAME * 1_000_000 + sample_rate as u64 / 2) / sample_rate as u64
}

pub fn pack_audio_chunk_payload(
    frame: &AudioFrame,
    timestamp_us: u64,
    duration_us: Option<u64>,
    sent_at_ms: u64,
) -> Vec<u8> {
    let description_b64 = if frame.include_config {
        Some(general_purpose::STANDARD.encode(&frame.audio_specific_config))
    } else {
        None
    };

    let meta = serde_json::json!({
        "type": "key",
        "timestamp": timestamp_us as i64,
        "duration": duration_us.map(|d| d as i64),
        "sentAt": sent_at_ms as i64,
        "codec": description_b64.as_ref().map(|_| "mp4a.40.2"),
        "descriptionBase64": description_b64,
        "sampleRate": frame.include_config.then_some(frame.sample_rate as i64),
        "channels": frame.include_config.then_some(frame.channels as i64),
    });
    let meta_bytes = meta.to_string().into_bytes();
    let meta_len = meta_bytes.len() as u32;

    let mut out = Vec::with_capacity(4 + meta_bytes.len() + frame.data.len());
    out.extend_from_slice(&meta_len.to_be_bytes());
    out.extend_from_slice(&meta_bytes);
    out.extend_from_slice(&frame.data);
    out
}

impl AacState {
    pub fn handle_flv_audio(&mut self, data: &[u8]) -> Result<Option<AudioFrame>> {
        if data.len() < 2 {
            return Ok(None);
        }

        let sound_format = data[0] >> 4;
        if sound_format != 10 {
            return Ok(None);
        }
        let packet_type = data[1];
        let payload = &data[2..];

        match packet_type {
            0 => {
                self.parse_audio_specific_config(payload)?;
                self.config_sent = false;
                Ok(None)
            }
            1 => {
                let (sample_rate, channels, asc) = match (
                    self.sample_rate,
                    self.channels,
                    self.audio_specific_config.as_ref(),
                ) {
                    (Some(sr), Some(ch), Some(asc)) => (sr, ch, asc.clone()),
                    _ => return Ok(None),
                };
                let include_config = !self.config_sent;
                self.config_sent = true;
                Ok(Some(AudioFrame {
                    data: payload.to_vec(),
                    sample_rate,
                    channels,
                    include_config,
                    audio_specific_config: asc,
                }))
            }
            _ => Ok(None),
        }
    }

    fn parse_audio_specific_config(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() < 2 {
            return Ok(());
        }
        let audio_object_type = (payload[0] >> 3) & 0x1f;
        if audio_object_type == 31 && payload.len() >= 3 {
            // escape code path (not expected here)
            return Ok(());
        }

        let sampling_frequency_index = ((payload[0] & 0x07) << 1) | ((payload[1] >> 7) & 0x01);
        let channel_config = (payload[1] >> 3) & 0x0f;

        if let Some(sr) = sampling_frequency_from_index(sampling_frequency_index as usize) {
            self.sample_rate = Some(sr);
        }
        self.channels = Some(channel_config);
        self.audio_specific_config = Some(payload.to_vec());
        Ok(())
    }
}

fn sampling_frequency_from_index(idx: usize) -> Option<u32> {
    const TABLE: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    TABLE.get(idx).copied()
}

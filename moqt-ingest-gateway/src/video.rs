use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub struct AvcState {
    length_size: usize,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
}

#[derive(serde::Serialize)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub is_key: bool,
}

/// js/examples/media に合わせた payload 生成: [meta_len(4byte BE)][meta json][Annex-B data]
pub fn pack_video_chunk_payload(
    is_key: bool,
    timestamp_us: u64,
    sent_at_ms: u64,
    data: &[u8],
) -> Vec<u8> {
    let meta = serde_json::json!({
        "type": if is_key { "key" } else { "delta" },
        "timestamp": timestamp_us as i64,
        "duration": 0i64, // duration null 相当
        "sentAt": sent_at_ms as i64,
    });
    let meta_bytes = meta.to_string().into_bytes();
    let meta_len = meta_bytes.len() as u32;

    let mut out = Vec::with_capacity(4 + meta_bytes.len() + data.len());
    out.extend_from_slice(&meta_len.to_be_bytes());
    out.extend_from_slice(&meta_bytes);
    out.extend_from_slice(data);
    out
}

impl AvcState {
    pub fn handle_flv_video(&mut self, data: &[u8]) -> Result<Option<VideoFrame>> {
        if data.len() < 5 {
            return Ok(None);
        }
        let codec_id = data[0] & 0x0f;
        if codec_id != 7 {
            return Ok(None);
        }
        let frame_type = data[0] >> 4;
        let avc_packet_type = data[1];
        let payload = &data[5..];
        match avc_packet_type {
            0 => {
                self.parse_config(payload)?;
                Ok(None)
            }
            1 => {
                if self.length_size == 0 {
                    return Ok(None);
                }
                let mut out = Vec::with_capacity(payload.len() + 64);
                let is_key = frame_type == 1;
                if is_key {
                    for sps in &self.sps {
                        out.extend_from_slice(&[0, 0, 0, 1]);
                        out.extend_from_slice(sps);
                    }
                    for pps in &self.pps {
                        out.extend_from_slice(&[0, 0, 0, 1]);
                        out.extend_from_slice(pps);
                    }
                }

                let mut pos = 0;
                while pos + self.length_size <= payload.len() {
                    let mut len_bytes = [0u8; 4];
                    len_bytes[4 - self.length_size..]
                        .copy_from_slice(&payload[pos..pos + self.length_size]);
                    pos += self.length_size;
                    let nal_len = u32::from_be_bytes(len_bytes) as usize;
                    if pos + nal_len > payload.len() {
                        break;
                    }
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(&payload[pos..pos + nal_len]);
                    pos += nal_len;
                }
                Ok(Some(VideoFrame { data: out, is_key }))
            }
            _ => Ok(None),
        }
    }

    fn parse_config(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() < 7 {
            return Ok(());
        }
        self.length_size = (payload[4] & 0b11) as usize + 1;
        let mut pos = 5;
        let num_sps = (payload[pos] & 0x1f) as usize;
        pos += 1;
        self.sps.clear();
        for _ in 0..num_sps {
            if pos + 2 > payload.len() {
                break;
            }
            let len = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
            pos += 2;
            if pos + len > payload.len() {
                break;
            }
            self.sps.push(payload[pos..pos + len].to_vec());
            pos += len;
        }

        if pos >= payload.len() {
            return Ok(());
        }
        let num_pps = payload[pos] as usize;
        pos += 1;
        self.pps.clear();
        for _ in 0..num_pps {
            if pos + 2 > payload.len() {
                break;
            }
            let len = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
            pos += 2;
            if pos + len > payload.len() {
                break;
            }
            self.pps.push(payload[pos..pos + len].to_vec());
            pos += len;
        }

        Ok(())
    }
}

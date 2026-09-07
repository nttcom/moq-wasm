use anyhow::{Result, ensure};
use bytes::{Bytes, BytesMut};

use crate::aac::asc::{AudioSpecificConfig, SAMPLE_RATES, sampling_frequency_index};

pub const HEADER_LENGTH: usize = 7;
const MAX_FRAME_LENGTH: usize = (1 << 13) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdtsHeader {
    pub config: AudioSpecificConfig,
    pub frame_length: usize,
    pub header_length: usize,
}

fn has_sync_word(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] & 0xF6 == 0xF0
}

pub fn parse_header(data: &[u8]) -> Result<Option<AdtsHeader>> {
    if data.len() < HEADER_LENGTH {
        return Ok(None);
    }
    ensure!(has_sync_word(data), "missing ADTS sync word");
    let protection_absent = data[1] & 1 == 1;
    let object_type = (data[2] >> 6) + 1;
    let frequency_index = ((data[2] >> 2) & 0x0F) as usize;
    let sample_rate = *SAMPLE_RATES.get(frequency_index).ok_or_else(|| {
        anyhow::anyhow!("invalid ADTS sampling frequency index {frequency_index}")
    })?;
    let channel_configuration = ((data[2] & 1) << 2) | (data[3] >> 6);
    let frame_length =
        ((data[3] & 0x03) as usize) << 11 | (data[4] as usize) << 3 | (data[5] >> 5) as usize;
    let header_length = if protection_absent { 7 } else { 9 };
    ensure!(
        frame_length >= header_length,
        "ADTS frame length {frame_length} shorter than its header"
    );
    Ok(Some(AdtsHeader {
        config: AudioSpecificConfig::new(object_type, sample_rate, channel_configuration),
        frame_length,
        header_length,
    }))
}

pub fn write_header(config: &AudioSpecificConfig, payload_length: usize) -> Result<[u8; 7]> {
    ensure!(
        (1..=4).contains(&config.object_type),
        "ADTS cannot signal audio object type {}",
        config.object_type
    );
    let frequency_index = sampling_frequency_index(config.sample_rate)
        .ok_or_else(|| anyhow::anyhow!("ADTS cannot signal sample rate {}", config.sample_rate))?;
    let frame_length = payload_length + HEADER_LENGTH;
    ensure!(
        frame_length <= MAX_FRAME_LENGTH,
        "ADTS frame too long: {frame_length}"
    );
    let channels = config.channel_configuration;
    Ok([
        0xFF,
        0xF1,
        ((config.object_type - 1) << 6) | (frequency_index << 2) | (channels >> 2),
        ((channels & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03),
        (frame_length >> 3) as u8,
        ((frame_length as u8 & 0x07) << 5) | 0x1F,
        0xFC,
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdtsFrame {
    pub config: AudioSpecificConfig,
    pub data: Bytes,
}

#[derive(Default)]
pub struct AdtsReader {
    buffer: BytesMut,
}

impl AdtsReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<AdtsFrame>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if !has_sync_word(&self.buffer) {
                self.discard_until_sync_word();
            }
            let Some(header) = parse_header(&self.buffer)? else {
                break;
            };
            if self.buffer.len() < header.frame_length {
                break;
            }
            let frame = self.buffer.split_to(header.frame_length).freeze();
            frames.push(AdtsFrame {
                config: header.config,
                data: frame.slice(header.header_length..),
            });
        }
        Ok(frames)
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    fn discard_until_sync_word(&mut self) {
        let sync = (0..self.buffer.len()).find(|index| has_sync_word(&self.buffer[*index..]));
        let keep_from = sync.unwrap_or(self.buffer.len().saturating_sub(1));
        let _ = self.buffer.split_to(keep_from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_48k() -> AudioSpecificConfig {
        AudioSpecificConfig::new(2, 48_000, 2)
    }

    fn adts_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = write_header(&stereo_48k(), payload.len()).unwrap().to_vec();
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn header_round_trips() {
        // Arrange
        let frame = adts_frame(&[1, 2, 3]);

        // Act
        let header = parse_header(&frame).unwrap().unwrap();

        // Assert
        assert_eq!(header.config, stereo_48k());
        assert_eq!(header.frame_length, 10);
        assert_eq!(header.header_length, 7);
    }

    #[test]
    fn reader_splits_frames_across_pushes() {
        // Arrange
        let mut stream = adts_frame(&[1, 2, 3]);
        stream.extend(adts_frame(&[4, 5]));
        let mut reader = AdtsReader::new();

        // Act
        let first = reader.push(&stream[..12]).unwrap();
        let second = reader.push(&stream[12..]).unwrap();

        // Assert
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].data.as_ref(), [1, 2, 3]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].data.as_ref(), [4, 5]);
    }

    #[test]
    fn reader_resynchronizes_after_garbage() {
        // Arrange
        let mut stream = vec![0x12, 0x34, 0x56];
        stream.extend(adts_frame(&[9]));
        let mut reader = AdtsReader::new();

        // Act
        let frames = reader.push(&stream).unwrap();

        // Assert
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data.as_ref(), [9]);
    }
}

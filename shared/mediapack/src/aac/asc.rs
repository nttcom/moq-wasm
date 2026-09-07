use anyhow::{Result, ensure};
use bytes::Bytes;

use crate::{
    bits::{BitReader, BitWriter},
    sample::Timestamp,
};

pub const SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

pub const AAC_LC: u8 = 2;
pub const SAMPLES_PER_FRAME: u64 = 1_024;

/// `bytes` keeps the serialized form the config was parsed from, so extension
/// signaling (SBR/PS) survives even though only the core fields are decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    pub object_type: u8,
    pub sample_rate: u32,
    pub channel_configuration: u8,
    bytes: Bytes,
}

impl AudioSpecificConfig {
    pub fn new(object_type: u8, sample_rate: u32, channel_configuration: u8) -> Self {
        let mut writer = BitWriter::new();
        if object_type >= 31 {
            writer.write_bits(31, 5);
            writer.write_bits((object_type - 32) as u64, 6);
        } else {
            writer.write_bits(object_type as u64, 5);
        }
        match sampling_frequency_index(sample_rate) {
            Some(index) => writer.write_bits(index as u64, 4),
            None => {
                writer.write_bits(15, 4);
                writer.write_bits(sample_rate as u64, 24);
            }
        }
        writer.write_bits(channel_configuration as u64, 4);
        writer.write_bits(0, 3);
        Self {
            object_type,
            sample_rate,
            channel_configuration,
            bytes: Bytes::from(writer.finish()),
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = BitReader::new(data);
        let mut object_type = reader.read_bits(5)? as u8;
        if object_type == 31 {
            object_type = 32 + reader.read_bits(6)? as u8;
        }
        let frequency_index = reader.read_bits(4)? as usize;
        let sample_rate = if frequency_index == 15 {
            reader.read_bits(24)? as u32
        } else {
            *SAMPLE_RATES.get(frequency_index).ok_or_else(|| {
                anyhow::anyhow!("invalid sampling frequency index {frequency_index}")
            })?
        };
        let channel_configuration = reader.read_bits(4)? as u8;
        ensure!(object_type != 0, "invalid audio object type 0");
        Ok(Self {
            object_type,
            sample_rate,
            channel_configuration,
            bytes: Bytes::copy_from_slice(data),
        })
    }

    pub fn to_bytes(&self) -> Bytes {
        self.bytes.clone()
    }

    pub fn codec_string(&self) -> String {
        format!("mp4a.40.{}", self.object_type)
    }

    pub fn channel_count(&self) -> u8 {
        match self.channel_configuration {
            7 => 8,
            count => count,
        }
    }

    pub fn frame_duration(&self) -> Timestamp {
        Timestamp::from_ticks(SAMPLES_PER_FRAME, self.sample_rate)
    }
}

pub fn sampling_frequency_index(sample_rate: u32) -> Option<u8> {
    SAMPLE_RATES
        .iter()
        .position(|rate| *rate == sample_rate)
        .map(|index| index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aac_lc_stereo_config() {
        // Arrange
        let bytes = [0x11, 0x90];

        // Act
        let config = AudioSpecificConfig::parse(&bytes).unwrap();

        // Assert
        assert_eq!(config, AudioSpecificConfig::new(AAC_LC, 48_000, 2));
        assert_eq!(config.codec_string(), "mp4a.40.2");
        assert_eq!(config.frame_duration().micros(), 21_333);
    }

    #[test]
    fn serializes_config_back_to_two_bytes() {
        // Arrange
        let config = AudioSpecificConfig::new(AAC_LC, 44_100, 1);

        // Act
        let bytes = config.to_bytes();

        // Assert
        assert_eq!(bytes.as_ref(), [0x12, 0x08]);
        assert_eq!(AudioSpecificConfig::parse(&bytes).unwrap(), config);
    }

    #[test]
    fn keeps_extended_config_bytes_from_parsed_input() {
        // Arrange
        let with_sbr_signaling = [0x11, 0x90, 0x56, 0xE5, 0x00];

        // Act
        let config = AudioSpecificConfig::parse(&with_sbr_signaling).unwrap();

        // Assert
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.channel_configuration, 2);
        assert_eq!(config.to_bytes().as_ref(), with_sbr_signaling);
    }

    #[test]
    fn round_trips_explicit_sample_rate_and_escaped_object_type() {
        // Arrange
        let config = AudioSpecificConfig::new(42, 50_000, 7);

        // Act
        let parsed = AudioSpecificConfig::parse(&config.to_bytes()).unwrap();

        // Assert
        assert_eq!(parsed, config);
        assert_eq!(parsed.channel_count(), 8);
    }
}

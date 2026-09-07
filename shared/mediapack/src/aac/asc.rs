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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    pub object_type: u8,
    pub sample_rate: u32,
    pub channel_configuration: u8,
}

impl AudioSpecificConfig {
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
        })
    }

    pub fn to_bytes(&self) -> Bytes {
        let mut writer = BitWriter::new();
        if self.object_type >= 31 {
            writer.write_bits(31, 5);
            writer.write_bits((self.object_type - 32) as u64, 6);
        } else {
            writer.write_bits(self.object_type as u64, 5);
        }
        match sampling_frequency_index(self.sample_rate) {
            Some(index) => writer.write_bits(index as u64, 4),
            None => {
                writer.write_bits(15, 4);
                writer.write_bits(self.sample_rate as u64, 24);
            }
        }
        writer.write_bits(self.channel_configuration as u64, 4);
        writer.write_bits(0, 3);
        Bytes::from(writer.finish())
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
        assert_eq!(
            config,
            AudioSpecificConfig {
                object_type: AAC_LC,
                sample_rate: 48_000,
                channel_configuration: 2
            }
        );
        assert_eq!(config.codec_string(), "mp4a.40.2");
        assert_eq!(config.frame_duration().micros(), 21_333);
    }

    #[test]
    fn serializes_config_back_to_two_bytes() {
        // Arrange
        let config = AudioSpecificConfig {
            object_type: AAC_LC,
            sample_rate: 44_100,
            channel_configuration: 1,
        };

        // Act
        let bytes = config.to_bytes();

        // Assert
        assert_eq!(bytes.as_ref(), [0x12, 0x08]);
        assert_eq!(AudioSpecificConfig::parse(&bytes).unwrap(), config);
    }

    #[test]
    fn round_trips_explicit_sample_rate_and_escaped_object_type() {
        // Arrange
        let config = AudioSpecificConfig {
            object_type: 42,
            sample_rate: 50_000,
            channel_configuration: 7,
        };

        // Act
        let parsed = AudioSpecificConfig::parse(&config.to_bytes()).unwrap();

        // Assert
        assert_eq!(parsed, config);
        assert_eq!(parsed.channel_count(), 8);
    }
}

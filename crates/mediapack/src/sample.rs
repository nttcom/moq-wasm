use bytes::Bytes;

use crate::{aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Timestamp {
    micros: u64,
}

impl Timestamp {
    pub const ZERO: Self = Self { micros: 0 };

    pub const fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    pub const fn from_millis(millis: u64) -> Self {
        Self {
            micros: millis * 1_000,
        }
    }

    pub fn from_ticks(ticks: u64, timescale: u32) -> Self {
        Self {
            micros: (ticks as u128 * 1_000_000 / timescale as u128) as u64,
        }
    }

    pub const fn micros(self) -> u64 {
        self.micros
    }

    pub const fn millis(self) -> u64 {
        self.micros / 1_000
    }

    pub fn ticks(self, timescale: u32) -> u64 {
        (self.micros as u128 * timescale as u128 / 1_000_000) as u64
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            micros: self.micros.saturating_add(other.micros),
        }
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            micros: self.micros.saturating_sub(other.micros),
        }
    }
}

/// `data` is an Annex-B access unit; keyframes carry their SPS/PPS in-band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSample {
    pub data: Bytes,
    pub is_keyframe: bool,
    pub pts: Timestamp,
    pub dts: Timestamp,
}

/// `data` is one raw AAC access unit without an ADTS header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSample {
    pub data: Bytes,
    pub pts: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamSet {
    pub has_video: bool,
    pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    Streams(StreamSet),
    VideoConfig(AvcDecoderConfigurationRecord),
    AudioConfig(AudioSpecificConfig),
    Video(VideoSample),
    Audio(AudioSample),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_ticks_and_micros() {
        // Arrange
        let pts_90khz = 127_920_u64;

        // Act
        let timestamp = Timestamp::from_ticks(pts_90khz, 90_000);

        // Assert
        assert_eq!(timestamp.micros(), 1_421_333);
        assert_eq!(timestamp.millis(), 1_421);
        assert_eq!(timestamp.ticks(90_000), 127_919);
        assert_eq!(Timestamp::from_millis(1_421).ticks(48_000), 68_208);
    }
}

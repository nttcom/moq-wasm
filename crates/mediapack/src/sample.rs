use anyhow::{Result, bail};
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

/// `data` is one encoded frame: a raw AAC access unit without an ADTS header,
/// A-law bytes, an Opus packet or an MP3 frame, as the `AudioConfig` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSample {
    pub data: Bytes,
    pub pts: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioConfig {
    Aac(AudioSpecificConfig),
    /// G.711 A-law (RFC 3551 §4.5.14).
    Pcma {
        sample_rate: u32,
        channels: u8,
    },
    Opus {
        sample_rate: u32,
        channels: u8,
    },
    /// MPEG-1/2 Audio Layer III.
    Mp3 {
        sample_rate: u32,
        channels: u8,
    },
}

impl AudioConfig {
    /// Fails unless the config is AAC.
    pub fn aac(&self) -> Result<&AudioSpecificConfig> {
        match self {
            Self::Aac(config) => Ok(config),
            other => bail!("{} audio is not supported here", other.codec_string()),
        }
    }

    /// The WebCodecs codec string.
    pub fn codec_string(&self) -> String {
        match self {
            Self::Aac(config) => config.codec_string(),
            Self::Pcma { .. } => "pcma".to_string(),
            Self::Opus { .. } => "opus".to_string(),
            Self::Mp3 { .. } => "mp3".to_string(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Aac(config) => config.sample_rate,
            Self::Pcma { sample_rate, .. }
            | Self::Opus { sample_rate, .. }
            | Self::Mp3 { sample_rate, .. } => *sample_rate,
        }
    }

    pub fn channels(&self) -> u8 {
        match self {
            Self::Aac(config) => config.channel_count(),
            Self::Pcma { channels, .. }
            | Self::Opus { channels, .. }
            | Self::Mp3 { channels, .. } => *channels,
        }
    }
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
    AudioConfig(AudioConfig),
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

    #[test]
    fn names_each_audio_codec_by_its_webcodecs_string() {
        // Arrange
        let aac = AudioConfig::Aac(AudioSpecificConfig::new(2, 48_000, 2));
        let pcma = AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        };
        let opus = AudioConfig::Opus {
            sample_rate: 48_000,
            channels: 2,
        };

        // Act / Assert
        assert_eq!(aac.codec_string(), "mp4a.40.2");
        assert_eq!(pcma.codec_string(), "pcma");
        assert_eq!(opus.codec_string(), "opus");
        assert_eq!((pcma.sample_rate(), pcma.channels()), (8_000, 1));
        assert_eq!((aac.sample_rate(), aac.channels()), (48_000, 2));
    }

    #[test]
    fn only_aac_has_an_audio_specific_config() {
        // Arrange
        let pcma = AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        };

        // Act
        let error = pcma.aac().unwrap_err();

        // Assert
        assert_eq!(error.to_string(), "pcma audio is not supported here");
    }
}

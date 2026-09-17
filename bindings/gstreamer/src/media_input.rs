use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::{
    AudioSample, MediaEvent, Timestamp, VideoSample, aac::AudioSpecificConfig,
    h264::ParameterSetTracker,
};

/// H.264 arrives as Annex-B access units. Parameter sets have to travel in band
/// (`h264parse config-interval=-1`): the first keyframe carrying them yields the
/// `VideoConfig` the catalog needs, and later keyframes get them re-inserted.
#[derive(Default)]
pub(crate) struct VideoInput {
    tracker: ParameterSetTracker,
    config_seen: bool,
}

impl VideoInput {
    pub(crate) fn push(
        &mut self,
        annexb: &[u8],
        pts: Timestamp,
        dts: Timestamp,
    ) -> Result<Vec<MediaEvent>> {
        let Some(unit) = self.tracker.track(annexb)? else {
            return Ok(Vec::new());
        };
        let mut events = Vec::with_capacity(2);
        if let Some(config) = unit.config_changed {
            self.config_seen = true;
            events.push(MediaEvent::VideoConfig(config));
        }
        if unit.is_keyframe && !self.config_seen {
            tracing::warn!(
                "keyframe without SPS/PPS in band; add `h264parse config-interval=-1` upstream"
            );
        }
        events.push(MediaEvent::Video(VideoSample {
            data: unit.data,
            is_keyframe: unit.is_keyframe,
            pts,
            dts,
        }));
        Ok(events)
    }
}

pub(crate) fn audio_config(codec_data: &[u8]) -> Result<MediaEvent> {
    let config = AudioSpecificConfig::parse(codec_data)
        .context("parse AudioSpecificConfig from the audio caps codec_data")?;
    Ok(MediaEvent::AudioConfig(config))
}

pub(crate) fn audio_sample(data: &[u8], pts: Timestamp) -> MediaEvent {
    MediaEvent::Audio(AudioSample {
        data: Bytes::copy_from_slice(data),
        pts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: [u8; 24] = [
        0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04,
        0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
    const START_CODE: [u8; 4] = [0, 0, 0, 1];

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| START_CODE.iter().chain(nal.iter()).copied())
            .collect()
    }

    #[test]
    fn emits_the_video_config_once_from_in_band_parameter_sets() {
        // Arrange
        let mut input = VideoInput::default();
        let keyframe = annexb(&[&SPS, &PPS, &[0x65, 0x88, 0x84]]);
        let delta = annexb(&[&[0x41, 0x9a, 0x02]]);

        // Act
        let first = input
            .push(&keyframe, Timestamp::ZERO, Timestamp::ZERO)
            .unwrap();
        let second = input
            .push(
                &delta,
                Timestamp::from_millis(33),
                Timestamp::from_millis(33),
            )
            .unwrap();

        // Assert
        assert!(matches!(first[0], MediaEvent::VideoConfig(_)));
        assert!(matches!(
            &first[1],
            MediaEvent::Video(sample) if sample.is_keyframe && sample.data == keyframe
        ));
        assert_eq!(second.len(), 1);
        assert!(matches!(
            &second[0],
            MediaEvent::Video(sample)
                if !sample.is_keyframe && sample.pts == Timestamp::from_millis(33)
        ));
    }

    #[test]
    fn reads_the_audio_config_from_codec_data() {
        // Arrange: AAC-LC, 48 kHz, stereo
        let codec_data = [0x11, 0x90];

        // Act
        let event = audio_config(&codec_data).unwrap();

        // Assert
        let MediaEvent::AudioConfig(config) = event else {
            panic!("expected an audio config");
        };
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.channel_count(), 2);
    }
}

use crate::{
    h264::annexb::{nal_units, with_start_codes},
    loc::object::{AUDIO_LEVEL_ID, LocExtension, LocObject, TIMESTAMP_ID},
    sample::{MediaEvent, Timestamp},
};

/// Emits the H.264 payload in Annex-B form, so parameter sets travel in band
/// and no video config extension is attached: a decoder configured from
/// extradata expects length-prefixed samples instead. Every start code is
/// rewritten to 4 bytes, as draft-ietf-moq-loc-04 §2.1.4 recommends, so a
/// receiver can tell it from a length prefix.
pub struct Muxer {
    capture_origin: Timestamp,
}

impl Muxer {
    /// `capture_origin` is the wall-clock time of presentation timestamp zero,
    /// since draft-ietf-moq-loc-04 §2.3.1.1 reads a Timestamp without a
    /// Timescale as microseconds since the Unix epoch.
    pub fn new(capture_origin: Timestamp) -> Self {
        Self { capture_origin }
    }

    /// An audio sample's level becomes the Audio Level property
    /// (draft-ietf-moq-loc-04 §2.3.3.2).
    pub fn push(&self, event: &MediaEvent) -> Option<LocObject> {
        let (payload, pts, audio_level) = match event {
            MediaEvent::Video(sample) => {
                (with_start_codes(nal_units(&sample.data)), sample.pts, None)
            }
            MediaEvent::Audio(sample) => (sample.data.clone(), sample.pts, sample.audio_level),
            MediaEvent::Streams(_) | MediaEvent::VideoConfig(_) | MediaEvent::AudioConfig(_) => {
                return None;
            }
        };

        let timestamp = LocExtension::varint(
            TIMESTAMP_ID,
            self.capture_origin.saturating_add(pts).micros(),
        );
        let audio_level = audio_level
            .map(|level| LocExtension::varint(AUDIO_LEVEL_ID, u64::from(level.to_byte())));
        Some(LocObject {
            extensions: [timestamp].into_iter().chain(audio_level).collect(),
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::{
        sample::{AudioLevel, AudioSample, StreamSet, VideoSample},
        test_support::{fixture_video_config, keyframe_annexb},
    };

    fn muxer() -> Muxer {
        Muxer::new(Timestamp::from_micros(1_700_000_000_000_000))
    }

    #[test]
    fn carries_the_video_payload_unchanged_with_a_capture_timestamp() {
        // Arrange
        let sample = VideoSample {
            data: keyframe_annexb(),
            is_keyframe: true,
            pts: Timestamp::from_millis(40),
            dts: Timestamp::from_millis(40),
        };

        // Act
        let object = muxer().push(&MediaEvent::Video(sample)).unwrap();

        // Assert
        assert_eq!(object.payload, keyframe_annexb());
        assert_eq!(
            object.capture_timestamp(),
            Some(Timestamp::from_micros(1_700_000_000_040_000))
        );
        assert_eq!(object.video_config(), None);
    }

    #[test]
    fn writes_every_start_code_with_four_bytes() {
        // Arrange
        let sample = VideoSample {
            data: Bytes::from_static(&[
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xCE, 0, 0, 1, 0x65, 0x88,
            ]),
            is_keyframe: true,
            pts: Timestamp::ZERO,
            dts: Timestamp::ZERO,
        };

        // Act
        let object = muxer().push(&MediaEvent::Video(sample)).unwrap();

        // Assert
        assert_eq!(
            object.payload.as_ref(),
            [
                0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0x88
            ]
        );
    }

    #[test]
    fn carries_the_audio_payload_unchanged() {
        // Arrange
        let sample = AudioSample {
            data: Bytes::from_static(&[1, 2, 3]),
            pts: Timestamp::from_millis(21),
            audio_level: None,
        };

        // Act
        let object = muxer().push(&MediaEvent::Audio(sample)).unwrap();

        // Assert
        assert_eq!(object.payload.as_ref(), [1, 2, 3]);
        assert_eq!(
            object.capture_timestamp(),
            Some(Timestamp::from_micros(1_700_000_000_021_000))
        );
    }

    #[test]
    fn writes_the_audio_level_as_its_property() {
        // Arrange
        let audio_level = AudioLevel {
            level: 45,
            voice_activity: true,
        };
        let sample = AudioSample {
            data: Bytes::from_static(&[1]),
            pts: Timestamp::ZERO,
            audio_level: Some(audio_level),
        };

        // Act
        let object = muxer().push(&MediaEvent::Audio(sample)).unwrap();

        // Assert
        assert_eq!(object.audio_level(), Some(audio_level));
    }

    #[test]
    fn produces_no_object_for_configuration_events() {
        // Arrange
        let muxer = muxer();

        // Act / Assert
        assert!(
            muxer
                .push(&MediaEvent::VideoConfig(fixture_video_config()))
                .is_none()
        );
        assert!(
            muxer
                .push(&MediaEvent::Streams(StreamSet {
                    has_video: true,
                    has_audio: false
                }))
                .is_none()
        );
    }
}

use crate::{
    loc::object::{CAPTURE_TIMESTAMP_ID, LocExtension, LocObject},
    sample::{MediaEvent, Timestamp},
};

/// Emits the H.264 payload in Annex-B form, so parameter sets travel in band
/// and no video config extension is attached: a decoder configured from
/// extradata expects length-prefixed samples instead.
pub struct Muxer {
    capture_origin: Timestamp,
}

impl Muxer {
    /// `capture_origin` is the wall-clock time of presentation timestamp zero,
    /// since draft-ietf-moq-loc-01 §2.3.1.1 defines the capture timestamp as
    /// microseconds since the Unix epoch.
    pub fn new(capture_origin: Timestamp) -> Self {
        Self { capture_origin }
    }

    pub fn push(&self, event: &MediaEvent) -> Option<LocObject> {
        let (payload, pts) = match event {
            MediaEvent::Video(sample) => (sample.data.clone(), sample.pts),
            MediaEvent::Audio(sample) => (sample.data.clone(), sample.pts),
            MediaEvent::Streams(_) | MediaEvent::VideoConfig(_) | MediaEvent::AudioConfig(_) => {
                return None;
            }
        };

        Some(LocObject {
            extensions: vec![LocExtension::varint(
                CAPTURE_TIMESTAMP_ID,
                self.capture_origin.saturating_add(pts).micros(),
            )],
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::{
        sample::{AudioSample, StreamSet, VideoSample},
        test_support::{fixture_record, keyframe_annexb},
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
    fn carries_the_audio_payload_unchanged() {
        // Arrange
        let sample = AudioSample {
            data: Bytes::from_static(&[1, 2, 3]),
            pts: Timestamp::from_millis(21),
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
    fn produces_no_object_for_configuration_events() {
        // Arrange
        let muxer = muxer();

        // Act / Assert
        assert!(
            muxer
                .push(&MediaEvent::VideoConfig(fixture_record()))
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

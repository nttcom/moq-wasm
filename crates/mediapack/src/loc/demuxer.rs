use anyhow::Result;
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    h264::{AvcDecoderConfigurationRecord, ParameterSetTracker, avcc::avcc_to_annexb},
    loc::object::LocObject,
    sample::{AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
};

const DEFAULT_NAL_LENGTH_SIZE: usize = 4;

/// A LOC track carries one codec, and draft-ietf-moq-loc-01 leaves the track's
/// kind and its audio configuration to the catalog, so the caller states them.
pub struct Demuxer {
    track: Track,
    capture_origin: Option<Timestamp>,
    last_pts: Timestamp,
    announced: bool,
}

enum Track {
    Video(ParameterSetTracker),
    Audio(AudioSpecificConfig),
}

impl Demuxer {
    pub fn video() -> Self {
        Self {
            track: Track::Video(ParameterSetTracker::new()),
            capture_origin: None,
            last_pts: Timestamp::ZERO,
            announced: false,
        }
    }

    pub fn audio(config: AudioSpecificConfig) -> Self {
        Self {
            track: Track::Audio(config),
            capture_origin: None,
            last_pts: Timestamp::ZERO,
            announced: false,
        }
    }

    pub fn push(&mut self, object: &LocObject) -> Result<Vec<MediaEvent>> {
        let mut events = Vec::new();
        if !self.announced {
            self.announced = true;
            events.push(MediaEvent::Streams(StreamSet {
                has_video: matches!(self.track, Track::Video(_)),
                has_audio: matches!(self.track, Track::Audio(_)),
            }));
            if let Track::Audio(config) = &self.track {
                events.push(MediaEvent::AudioConfig(config.clone()));
            }
        }

        let pts = self.presentation_timestamp(object);
        match &mut self.track {
            Track::Video(parameter_sets) => {
                let annexb = to_annexb(object)?;
                let Some(unit) = parameter_sets.track(&annexb)? else {
                    return Ok(events);
                };
                if let Some(config) = unit.config_changed {
                    events.push(MediaEvent::VideoConfig(config));
                }
                events.push(MediaEvent::Video(VideoSample {
                    data: unit.data,
                    is_keyframe: unit.is_keyframe,
                    pts,
                    dts: pts,
                }));
            }
            Track::Audio(_) => {
                events.push(MediaEvent::Audio(AudioSample {
                    data: object.payload.clone(),
                    pts,
                }));
            }
        }
        Ok(events)
    }

    fn presentation_timestamp(&mut self, object: &LocObject) -> Timestamp {
        let Some(capture) = object.capture_timestamp() else {
            return self.last_pts;
        };
        let origin = *self.capture_origin.get_or_insert(capture);
        self.last_pts = capture.saturating_sub(origin);
        self.last_pts
    }
}

/// draft-ietf-moq-loc-01 §2.1.3 lets a length prefix of 1 be read as a start
/// code, so a leading start code identifies an Annex-B payload.
fn to_annexb(object: &LocObject) -> Result<Bytes> {
    let payload = &object.payload;
    if payload.starts_with(&[0, 0, 0, 1]) || payload.starts_with(&[0, 0, 1]) {
        return Ok(payload.clone());
    }
    avcc_to_annexb(payload, nal_length_size(object))
}

fn nal_length_size(object: &LocObject) -> usize {
    object
        .video_config()
        .and_then(|config| AvcDecoderConfigurationRecord::parse(&config).ok())
        .map_or(DEFAULT_NAL_LENGTH_SIZE, |config| {
            config.nal_length_size as usize
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h264::annexb::annexb_to_avcc,
        loc::object::{CAPTURE_TIMESTAMP_ID, LocExtension, VIDEO_CONFIG_ID},
        test_support::{delta_frame_annexb, fixture_record, keyframe_annexb, mono_48k},
    };

    fn video_object(pts_micros: u64, payload: Bytes) -> LocObject {
        LocObject {
            extensions: vec![LocExtension::varint(CAPTURE_TIMESTAMP_ID, pts_micros)],
            payload,
        }
    }

    #[test]
    fn announces_the_track_and_reports_the_configuration_from_the_payload() {
        // Arrange
        let mut demuxer = Demuxer::video();

        // Act
        let events = demuxer
            .push(&video_object(1_000_000, keyframe_annexb()))
            .unwrap();

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: false
            })
        );
        assert_eq!(events[1], MediaEvent::VideoConfig(fixture_record()));
        assert_eq!(
            events[2],
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::ZERO,
                dts: Timestamp::ZERO
            })
        );
    }

    #[test]
    fn measures_timestamps_from_the_first_capture_timestamp() {
        // Arrange
        let mut demuxer = Demuxer::video();
        demuxer
            .push(&video_object(1_000_000, keyframe_annexb()))
            .unwrap();

        // Act
        let events = demuxer
            .push(&video_object(1_040_000, delta_frame_annexb()))
            .unwrap();

        // Assert
        assert_eq!(
            events,
            [MediaEvent::Video(VideoSample {
                data: delta_frame_annexb(),
                is_keyframe: false,
                pts: Timestamp::from_millis(40),
                dts: Timestamp::from_millis(40)
            })]
        );
    }

    #[test]
    fn converts_a_length_prefixed_payload_using_the_video_config_extension() {
        // Arrange
        let mut config = fixture_record();
        config.nal_length_size = 2;
        let mut demuxer = Demuxer::video();
        let object = LocObject {
            extensions: vec![
                LocExtension::varint(CAPTURE_TIMESTAMP_ID, 0),
                LocExtension::bytes(VIDEO_CONFIG_ID, config.to_bytes()),
            ],
            payload: annexb_to_avcc(&keyframe_annexb(), 2),
        };

        // Act
        let events = demuxer.push(&object).unwrap();

        // Assert
        assert_eq!(
            events.last(),
            Some(&MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::ZERO,
                dts: Timestamp::ZERO
            }))
        );
    }

    #[test]
    fn keeps_the_previous_timestamp_when_an_object_carries_none() {
        // Arrange
        let mut demuxer = Demuxer::video();
        demuxer
            .push(&video_object(5_000_000, keyframe_annexb()))
            .unwrap();

        // Act
        let events = demuxer
            .push(&LocObject {
                extensions: Vec::new(),
                payload: delta_frame_annexb(),
            })
            .unwrap();

        // Assert
        let MediaEvent::Video(sample) = &events[0] else {
            panic!("expected a video sample, got {:?}", events[0]);
        };
        assert_eq!(sample.pts, Timestamp::ZERO);
    }

    #[test]
    fn round_trips_the_samples_a_transport_stream_carries() {
        // Arrange
        let mut source = crate::mpegts::Demuxer::new();
        let mut events = source.push(crate::test_support::FIXTURE_TS).unwrap();
        events.extend(source.finish().unwrap());
        let expected = crate::test_support::video_samples(&events);
        let muxer = crate::loc::Muxer::new(Timestamp::from_micros(1_700_000_000_000_000));
        let mut demuxer = Demuxer::video();

        // Act
        let mut decoded = Vec::new();
        for event in &events {
            let Some(object) = muxer.push(event) else {
                continue;
            };
            if matches!(event, MediaEvent::Video(_)) {
                decoded.extend(demuxer.push(&object).unwrap());
            }
        }

        // Assert
        let actual = crate::test_support::video_samples(&decoded);
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(left, right)| left.data == right.data
                    && left.is_keyframe == right.is_keyframe)
        );
        let first_pts = expected[0].pts;
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(left, right)| left.pts == right.pts.saturating_sub(first_pts))
        );
    }

    #[test]
    fn reports_the_audio_configuration_the_caller_supplied() {
        // Arrange
        let mut demuxer = Demuxer::audio(mono_48k());
        let object = LocObject {
            extensions: vec![LocExtension::varint(CAPTURE_TIMESTAMP_ID, 2_000_000)],
            payload: Bytes::from_static(&[7, 7]),
        };

        // Act
        let events = demuxer.push(&object).unwrap();
        let subsequent = demuxer.push(&object).unwrap();

        // Assert
        assert!(matches!(subsequent.as_slice(), [MediaEvent::Audio(_)]));
        assert_eq!(
            events,
            [
                MediaEvent::Streams(StreamSet {
                    has_video: false,
                    has_audio: true
                }),
                MediaEvent::AudioConfig(mono_48k()),
                MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[7, 7]),
                    pts: Timestamp::ZERO
                })
            ]
        );
    }
}

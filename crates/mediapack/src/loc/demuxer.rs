use anyhow::Result;
use bytes::Bytes;

use crate::{
    av1::{self, Av1Config},
    h264::{AvcDecoderConfigurationRecord, ParameterSetTracker, avcc::avcc_to_annexb},
    loc::object::LocObject,
    sample::{
        AudioConfig, AudioSample, MediaEvent, StreamSet, Timestamp, VideoConfig, VideoSample,
    },
    vp8::{self, Vp8Config},
    vp9::{self, Vp9Config},
};

const DEFAULT_NAL_LENGTH_SIZE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Vp8,
    Vp9,
    Av1,
}

impl VideoCodec {
    /// Reads a WebCodecs codec string, the form a catalog's `codec` takes;
    /// `None` for any other codec.
    pub fn from_codec_string(codec: &str) -> Option<Self> {
        let family = codec.split('.').next().unwrap_or_default();
        match family {
            "avc1" | "avc3" => Some(Self::H264),
            "vp8" => Some(Self::Vp8),
            "vp09" => Some(Self::Vp9),
            "av01" => Some(Self::Av1),
            _ => None,
        }
    }
}

/// A LOC track carries one codec, and draft-ietf-moq-loc-01 leaves the track's
/// kind, its codec and its audio configuration to the catalog, so the caller
/// states them.
pub struct Demuxer {
    track: Track,
    capture_origin: Option<Timestamp>,
    last_pts: Timestamp,
    announced: bool,
}

/// The VP8, VP9 and AV1 variants hold the configuration last reported.
enum Track {
    H264(ParameterSetTracker),
    Vp8(Option<Vp8Config>),
    Vp9(Option<Vp9Config>),
    Av1(Option<Av1Config>),
    Audio(AudioConfig),
}

impl Demuxer {
    pub fn video(codec: VideoCodec) -> Self {
        Self::new(match codec {
            VideoCodec::H264 => Track::H264(ParameterSetTracker::new()),
            VideoCodec::Vp8 => Track::Vp8(None),
            VideoCodec::Vp9 => Track::Vp9(None),
            VideoCodec::Av1 => Track::Av1(None),
        })
    }

    pub fn audio(config: AudioConfig) -> Self {
        Self::new(Track::Audio(config))
    }

    fn new(track: Track) -> Self {
        Self {
            track,
            capture_origin: None,
            last_pts: Timestamp::ZERO,
            announced: false,
        }
    }

    /// Measures presentation time from `capture_origin` instead of the first
    /// capture timestamp seen on this track, so the demuxers of the tracks of
    /// one broadcast share one origin and their samples stay aligned.
    pub fn with_capture_origin(mut self, capture_origin: Timestamp) -> Self {
        self.capture_origin = Some(capture_origin);
        self
    }

    pub fn push(&mut self, object: &LocObject) -> Result<Vec<MediaEvent>> {
        let mut events = Vec::new();
        if !self.announced {
            self.announced = true;
            let has_audio = matches!(self.track, Track::Audio(_));
            events.push(MediaEvent::Streams(StreamSet {
                has_video: !has_audio,
                has_audio,
            }));
            if let Track::Audio(config) = &self.track {
                events.push(MediaEvent::AudioConfig(config.clone()));
            }
        }

        let pts = self.presentation_timestamp(object);
        let payload = &object.payload;
        let (data, is_keyframe) = match &mut self.track {
            Track::H264(parameter_sets) => {
                let annexb = to_annexb(object)?;
                let Some(unit) = parameter_sets.track(&annexb)? else {
                    return Ok(events);
                };
                if let Some(config) = unit.config_changed {
                    events.push(MediaEvent::VideoConfig(VideoConfig::Avc(config)));
                }
                (unit.data, unit.is_keyframe)
            }
            Track::Vp8(current) => {
                let frame = vp8::parse_frame(payload)?;
                events.extend(config_change(current, frame.config, VideoConfig::Vp8));
                (payload.clone(), frame.is_keyframe)
            }
            Track::Vp9(current) => {
                let frame = vp9::parse_frame(payload)?;
                events.extend(config_change(current, frame.config, VideoConfig::Vp9));
                (payload.clone(), frame.is_keyframe)
            }
            Track::Av1(current) => {
                let unit = av1::parse_temporal_unit(payload, current.as_ref())?;
                events.extend(config_change(current, unit.config, VideoConfig::Av1));
                (payload.clone(), unit.is_keyframe)
            }
            Track::Audio(_) => {
                events.push(MediaEvent::Audio(AudioSample {
                    data: payload.clone(),
                    pts,
                }));
                return Ok(events);
            }
        };
        events.push(MediaEvent::Video(VideoSample {
            data,
            is_keyframe,
            pts,
            dts: pts,
        }));
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

/// Keeps `parsed` as the track's configuration and reports it when it differs
/// from the one reported before.
fn config_change<T: Clone + PartialEq>(
    current: &mut Option<T>,
    parsed: Option<T>,
    to_video_config: fn(T) -> VideoConfig,
) -> Option<MediaEvent> {
    let parsed = parsed.filter(|parsed| current.as_ref() != Some(parsed))?;
    *current = Some(parsed.clone());
    Some(MediaEvent::VideoConfig(to_video_config(parsed)))
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
        test_support::{
            FIXTURE_AV1_IVF, FIXTURE_VP8_IVF, FIXTURE_VP9_IVF, delta_frame_annexb, fixture_record,
            fixture_video_config, ivf_frames, keyframe_annexb, mono_48k, video_samples,
        },
    };

    fn video_object(pts_micros: u64, payload: Bytes) -> LocObject {
        LocObject {
            extensions: vec![LocExtension::varint(CAPTURE_TIMESTAMP_ID, pts_micros)],
            payload,
        }
    }

    #[test]
    fn measures_presentation_time_from_a_shared_origin() {
        // Arrange
        let mut demuxer =
            Demuxer::video(VideoCodec::H264).with_capture_origin(Timestamp::from_micros(1_000_000));

        // Act
        let events = demuxer
            .push(&video_object(1_500_000, keyframe_annexb()))
            .unwrap();

        // Assert
        assert_eq!(
            video_samples(&events)[0].pts,
            Timestamp::from_micros(500_000)
        );
    }

    #[test]
    fn announces_the_track_and_reports_the_configuration_from_the_payload() {
        // Arrange
        let mut demuxer = Demuxer::video(VideoCodec::H264);

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
        assert_eq!(events[1], MediaEvent::VideoConfig(fixture_video_config()));
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
        let mut demuxer = Demuxer::video(VideoCodec::H264);
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
        let mut demuxer = Demuxer::video(VideoCodec::H264);
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
        let mut demuxer = Demuxer::video(VideoCodec::H264);
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
        let mut demuxer = Demuxer::video(VideoCodec::H264);

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
    fn reports_the_configuration_of_each_codec_once_and_passes_its_frames_through() {
        for (codec, fixture) in [
            (VideoCodec::Vp8, FIXTURE_VP8_IVF),
            (VideoCodec::Vp9, FIXTURE_VP9_IVF),
            (VideoCodec::Av1, FIXTURE_AV1_IVF),
        ] {
            // Arrange: a keyframe, an inter frame, then the keyframe again
            let frames = ivf_frames(fixture);
            let payloads = [frames[0].clone(), frames[1].clone(), frames[0].clone()];
            let mut demuxer = Demuxer::video(codec);

            // Act
            let mut events = Vec::new();
            for (index, payload) in payloads.iter().enumerate() {
                events.extend(
                    demuxer
                        .push(&video_object(index as u64 * 40_000, payload.clone()))
                        .unwrap(),
                );
            }

            // Assert
            let configs: Vec<&VideoConfig> = events
                .iter()
                .filter_map(|event| match event {
                    MediaEvent::VideoConfig(config) => Some(config),
                    _ => None,
                })
                .collect();
            assert_eq!(configs.len(), 1, "{codec:?}");
            let samples = video_samples(&events);
            assert!(
                samples.iter().map(|sample| &sample.data).eq(&payloads),
                "{codec:?}"
            );
            assert_eq!(
                samples
                    .iter()
                    .map(|sample| sample.is_keyframe)
                    .collect::<Vec<_>>(),
                [true, false, true],
                "{codec:?}"
            );
        }
    }

    #[test]
    fn reads_the_codec_from_a_webcodecs_codec_string() {
        // Act / Assert
        assert_eq!(
            VideoCodec::from_codec_string("avc1.640028"),
            Some(VideoCodec::H264)
        );
        assert_eq!(VideoCodec::from_codec_string("vp8"), Some(VideoCodec::Vp8));
        assert_eq!(
            VideoCodec::from_codec_string("vp09.00.10.08"),
            Some(VideoCodec::Vp9)
        );
        assert_eq!(
            VideoCodec::from_codec_string("av01.0.08M.08"),
            Some(VideoCodec::Av1)
        );
        assert_eq!(VideoCodec::from_codec_string("hvc1.1.6.L93.B0"), None);
    }

    #[test]
    fn reports_a_keyframe_that_changes_the_frame_size() {
        // Arrange
        let keyframe = ivf_frames(FIXTURE_VP8_IVF)[0].clone();
        let mut resized = keyframe.to_vec();
        resized[6] = 32;
        let mut demuxer = Demuxer::video(VideoCodec::Vp8);
        demuxer.push(&video_object(0, keyframe)).unwrap();

        // Act
        let events = demuxer
            .push(&video_object(40_000, Bytes::from(resized)))
            .unwrap();

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::VideoConfig(VideoConfig::Vp8(Vp8Config {
                width: 32,
                height: 48
            }))
        );
    }

    #[test]
    fn reports_the_audio_configuration_the_caller_supplied() {
        // Arrange
        let mut demuxer = Demuxer::audio(AudioConfig::Aac(mono_48k()));
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
                MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())),
                MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[7, 7]),
                    pts: Timestamp::ZERO
                })
            ]
        );
    }
}

use std::time::Duration;

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use mediapack::{
    AudioConfig, MediaEvent, StreamSet, Timestamp, VideoConfig,
    loc::{self, LocObject},
    mp4::Fmp4Muxer,
};
use moqt::TrackObject;

/// What one object produced. `previous_file_tail` ends the file written so
/// far when a configuration change forces a new init segment or a segment
/// reached its length; `data` then belongs to the next file.
#[derive(Default)]
pub(crate) struct Output {
    pub(crate) previous_file_tail: Option<Bytes>,
    pub(crate) data: Bytes,
    /// The leading track moved to a new group, so every earlier group is
    /// complete.
    pub(crate) group_complete: bool,
}

pub(crate) struct FileTracks {
    pub(crate) video: Option<loc::VideoCodec>,
    pub(crate) audio: Option<AudioConfig>,
}

/// Each demuxer starts at its track's first object and counts presentation
/// time from the caller's capture origin. All files of a recording share that
/// origin, so they keep the offset between tracks.
pub(crate) struct Pipeline {
    video: Option<loc::Demuxer>,
    audio: Option<loc::Demuxer>,
    video_codec: Option<loc::VideoCodec>,
    audio_track: Option<AudioConfig>,
    streams: StreamSet,
    /// The tracks of the file the muxer writes. Lags `streams` after
    /// `add_audio` on a started file, until the next keyframe starts a new file.
    muxer_streams: StreamSet,
    muxer: Fmp4Muxer,
    video_config: Option<VideoConfig>,
    audio_config: Option<AudioConfig>,
    file_started: bool,
    /// Video before the first keyframe cannot be decoded and is dropped; a
    /// relay's oldest cached group can start mid-group.
    keyframe_seen: bool,
    /// Presentation time of the current file's first sample of the leading
    /// track: the video, or the audio in a file without video.
    file_start: Option<Timestamp>,
    segment_length: Option<Duration>,
    current_group: Option<u64>,
}

impl Pipeline {
    /// With a `segment_length`, a new file starts at the first keyframe at
    /// least that long after the current file's start; in a file without
    /// video, at the first audio frame.
    pub(crate) fn new(tracks: FileTracks, segment_length: Option<Duration>) -> Result<Self> {
        let streams = StreamSet {
            has_video: tracks.video.is_some(),
            has_audio: tracks.audio.is_some(),
        };
        Ok(Self {
            video: None,
            audio: None,
            video_codec: tracks.video,
            audio_track: tracks.audio.clone(),
            streams,
            muxer_streams: streams,
            muxer: new_muxer(streams, None, tracks.audio.as_ref())?,
            video_config: None,
            audio_config: tracks.audio,
            file_started: false,
            keyframe_seen: false,
            file_start: None,
            segment_length,
            current_group: None,
        })
    }

    pub(crate) fn push_video(
        &mut self,
        object: &TrackObject,
        capture_origin: Option<Timestamp>,
    ) -> Result<Output> {
        let codec = self
            .video_codec
            .context("video object for a file without video")?;
        let group_complete = self.group_complete(object);
        let loc_object = to_loc_object(object);
        let events = demuxer(&mut self.video, capture_origin, || {
            loc::Demuxer::video(codec)
        })
        .push(&loc_object)?;
        let mut output = self.mux(events)?;
        output.group_complete = group_complete;
        Ok(output)
    }

    /// Adds an audio track to a video-only pipeline. Takes effect at once
    /// while the file is still empty; otherwise the current file stays
    /// video-only, the next keyframe starts a file with both tracks, and audio
    /// before that keyframe is dropped.
    pub(crate) fn add_audio(&mut self, config: AudioConfig) -> Result<()> {
        self.streams.has_audio = true;
        self.audio_track = Some(config.clone());
        self.audio_config = Some(config);
        if !self.file_started {
            self.muxer = new_muxer(
                self.streams,
                self.video_config.as_ref(),
                self.audio_config.as_ref(),
            )?;
            self.muxer_streams = self.streams;
        }
        Ok(())
    }

    pub(crate) fn push_audio(
        &mut self,
        object: &TrackObject,
        capture_origin: Option<Timestamp>,
    ) -> Result<Output> {
        let config = self
            .audio_track
            .clone()
            .context("audio object for a file without audio")?;
        if !self.muxer_streams.has_audio {
            return Ok(Output::default());
        }
        let group_complete = self.video_codec.is_none() && self.group_complete(object);
        let loc_object = to_loc_object(object);
        let events = demuxer(&mut self.audio, capture_origin, || {
            loc::Demuxer::audio(config)
        })
        .push(&loc_object)?;
        let mut output = self.mux(events)?;
        output.group_complete = group_complete;
        Ok(output)
    }

    /// The fragment of the last video sample, whose duration was unknown
    /// until now.
    pub(crate) fn finish(&mut self) -> Result<Bytes> {
        self.muxer.finish()
    }

    /// Called with the objects of the leading track only.
    fn group_complete(&mut self, object: &TrackObject) -> bool {
        self.current_group
            .replace(object.group_id)
            .is_some_and(|previous| previous != object.group_id)
    }

    /// The demuxers' own `Streams` events are dropped: each announces its one
    /// track, while the muxer was told about every track of the file when the
    /// pipeline started.
    fn mux(&mut self, events: Vec<MediaEvent>) -> Result<Output> {
        let mut tail: Option<BytesMut> = None;
        let mut data = BytesMut::new();
        let has_video = self.streams.has_video;
        for event in events {
            if let MediaEvent::Video(sample) = &event {
                self.keyframe_seen |= sample.is_keyframe;
                if !self.keyframe_seen {
                    continue;
                }
            }
            let leading_sample = match &event {
                MediaEvent::Video(sample) if sample.is_keyframe => Some(sample.pts),
                MediaEvent::Audio(sample) if !has_video => Some(sample.pts),
                _ => None,
            };
            let changed = match &event {
                MediaEvent::Streams(_) => continue,
                MediaEvent::VideoConfig(config) => self.video_config.as_ref() != Some(config),
                MediaEvent::AudioConfig(config) => self.audio_config.as_ref() != Some(config),
                MediaEvent::Video(_) | MediaEvent::Audio(_) => leading_sample.is_some_and(|pts| {
                    self.muxer_streams != self.streams || self.segment_ended(pts)
                }),
            };
            if changed && self.file_started {
                tail.get_or_insert_default()
                    .extend_from_slice(&self.rotate(&mut data)?);
            }
            match &event {
                MediaEvent::VideoConfig(config) => self.video_config = Some(config.clone()),
                MediaEvent::AudioConfig(config) => self.audio_config = Some(config.clone()),
                MediaEvent::Video(sample) => {
                    self.file_start.get_or_insert(sample.pts);
                }
                MediaEvent::Audio(sample) if !has_video => {
                    self.file_start.get_or_insert(sample.pts);
                }
                _ => {}
            }
            data.extend_from_slice(&self.muxer.push(&event)?);
        }
        if !data.is_empty() {
            self.file_started = true;
        }
        Ok(Output {
            previous_file_tail: tail.map(BytesMut::freeze),
            data: data.freeze(),
            group_complete: false,
        })
    }

    fn segment_ended(&self, pts: Timestamp) -> bool {
        let (Some(length), Some(start)) = (self.segment_length, self.file_start) else {
            return false;
        };
        Duration::from_micros(pts.saturating_sub(start).micros()) >= length
    }

    /// Ends the current file with what `data` holds plus the pending video
    /// sample, and starts a muxer for the next one from the known
    /// configurations; the changed one is pushed by the caller right after.
    fn rotate(&mut self, data: &mut BytesMut) -> Result<Bytes> {
        let mut tail = std::mem::take(data);
        tail.extend_from_slice(&self.muxer.finish()?);
        self.muxer = new_muxer(
            self.streams,
            self.video_config.as_ref(),
            self.audio_config.as_ref(),
        )?;
        self.muxer_streams = self.streams;
        self.file_started = false;
        self.file_start = None;
        Ok(tail.freeze())
    }
}

pub(crate) fn capture_timestamp(object: &TrackObject) -> Option<Timestamp> {
    to_loc_object(object).capture_timestamp()
}

fn demuxer(
    slot: &mut Option<loc::Demuxer>,
    origin: Option<Timestamp>,
    create: impl FnOnce() -> loc::Demuxer,
) -> &mut loc::Demuxer {
    slot.get_or_insert_with(|| match origin {
        Some(origin) => create().with_capture_origin(origin),
        None => create(),
    })
}

fn new_muxer(
    streams: StreamSet,
    video_config: Option<&VideoConfig>,
    audio_config: Option<&AudioConfig>,
) -> Result<Fmp4Muxer> {
    let mut muxer = Fmp4Muxer::new();
    muxer.push(&MediaEvent::Streams(streams))?;
    if let Some(config) = video_config {
        muxer.push(&MediaEvent::VideoConfig(config.clone()))?;
    }
    if let Some(config) = audio_config {
        muxer.push(&MediaEvent::AudioConfig(config.clone()))?;
    }
    Ok(muxer)
}

fn to_loc_object(object: &TrackObject) -> LocObject {
    LocObject {
        extensions: loc::from_extension_headers(&object.extension_headers),
        payload: object.payload.clone(),
    }
}

#[cfg(test)]
mod tests {
    use mediapack::h264::AvcDecoderConfigurationRecord;

    use super::*;
    use crate::test_support::{
        OPUS_20_MS_FRAME, audio_samples, demux, first_video_capture, fixture, opus_config,
        opus_object, track_object, video_samples,
    };

    /// An IDR slice NAL in Annex-B form; only its NAL type is read.
    const IDR_SLICE: &[u8] = &[0, 0, 0, 1, 0x65, 0x88, 0x84];

    fn tracks(audio: Option<AudioConfig>) -> FileTracks {
        FileTracks {
            video: Some(loc::VideoCodec::H264),
            audio,
        }
    }

    #[test]
    fn writes_every_sample_of_both_tracks_into_one_file() {
        // Arrange: the audio of a group follows the first video object of the group
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(Some(fixture.audio_config.clone())), None).unwrap();
        let origin = capture_timestamp(&fixture.video[0]);
        let mut file = BytesMut::new();
        let mut groups_completed = 0;
        let mut audio = fixture.audio.iter().peekable();

        // Act
        for object in &fixture.video {
            let output = pipeline.push_video(object, origin).unwrap();
            assert_eq!(output.previous_file_tail, None);
            groups_completed += output.group_complete as usize;
            file.extend_from_slice(&output.data);
            while let Some(next) = audio.next_if(|audio| audio.group_id <= object.group_id) {
                file.extend_from_slice(&pipeline.push_audio(next, origin).unwrap().data);
            }
        }
        for next in audio {
            file.extend_from_slice(&pipeline.push_audio(next, origin).unwrap().data);
        }
        file.extend_from_slice(&pipeline.finish().unwrap());

        // Assert
        let recorded = demux(&file);
        let (expected_video, actual_video) =
            (video_samples(&fixture.events), video_samples(&recorded));
        assert_eq!(actual_video.len(), expected_video.len());
        assert_eq!(
            audio_samples(&recorded).len(),
            audio_samples(&fixture.events).len()
        );
        assert_eq!(
            groups_completed,
            fixture.video.last().unwrap().group_id as usize - 1
        );
        for (actual, expected) in actual_video.iter().zip(&expected_video) {
            assert_eq!(actual.data, expected.data);
            assert_eq!(actual.is_keyframe, expected.is_keyframe);
        }
    }

    #[test]
    fn keeps_the_offset_between_the_audio_and_the_video_timeline() {
        // Arrange: the audio track starts 30 ms after the first video object
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(Some(fixture.audio_config.clone())), None).unwrap();
        let origin = capture_timestamp(&fixture.video[0]);
        let first_video = fixture.video[0].clone();
        let mut late_audio = fixture.audio[0].clone();
        let video_capture = first_video_capture(&first_video);
        late_audio.extension_headers = loc::to_extension_headers(&[loc::LocExtension::varint(
            loc::CAPTURE_TIMESTAMP_ID,
            video_capture + 30_000,
        )]);
        let mut file = BytesMut::new();

        // Act
        file.extend_from_slice(&pipeline.push_video(&first_video, origin).unwrap().data);
        file.extend_from_slice(&pipeline.push_audio(&late_audio, origin).unwrap().data);
        file.extend_from_slice(&pipeline.push_video(&fixture.video[1], origin).unwrap().data);
        file.extend_from_slice(&pipeline.finish().unwrap());

        // Assert
        let recorded = demux(&file);
        let video = video_samples(&recorded);
        let audio = audio_samples(&recorded);
        assert_eq!(video[0].pts, Timestamp::ZERO);
        assert!(audio[0].pts.micros().abs_diff(30_000) <= 21);
    }

    #[test]
    fn records_opus_alaw_and_mp3_audio_beside_the_video() {
        // Arrange: (config, frame, frame µs); the fMP4 muxer derives each duration from the frame bytes
        let fixture = fixture();
        let cases = [
            (opus_config(), Bytes::from_static(&OPUS_20_MS_FRAME), 20_000),
            (
                AudioConfig::Pcma {
                    sample_rate: 8_000,
                    channels: 1,
                },
                Bytes::from(vec![0xD5; 160]),
                20_000,
            ),
            (
                AudioConfig::Mp3 {
                    sample_rate: 48_000,
                    channels: 2,
                },
                Bytes::from_static(&[0xFF, 0xFB, 0x94, 0x64, 0x00, 0x00]),
                24_000,
            ),
        ];
        let start = first_video_capture(&fixture.video[0]);
        for (config, frame, frame_micros) in cases {
            let mut pipeline = Pipeline::new(tracks(Some(config.clone())), None).unwrap();
            let origin = capture_timestamp(&fixture.video[0]);
            let mut file = BytesMut::new();

            // Act
            for object in &fixture.video {
                file.extend_from_slice(&pipeline.push_video(object, origin).unwrap().data);
            }
            for index in 0..10 {
                let audio = track_object(
                    1,
                    index,
                    LocObject {
                        extensions: vec![loc::LocExtension::varint(
                            loc::CAPTURE_TIMESTAMP_ID,
                            start + index * frame_micros,
                        )],
                        payload: frame.clone(),
                    },
                );
                file.extend_from_slice(&pipeline.push_audio(&audio, origin).unwrap().data);
            }
            file.extend_from_slice(&pipeline.finish().unwrap());

            // Assert
            let recorded = demux(&file);
            assert!(
                recorded.contains(&MediaEvent::AudioConfig(config.clone())),
                "{config:?}"
            );
            assert_eq!(
                video_samples(&recorded).len(),
                video_samples(&fixture.events).len()
            );
            let audio = audio_samples(&recorded);
            assert_eq!(audio.len(), 10, "{config:?}");
            for (index, sample) in audio.iter().enumerate() {
                assert_eq!(sample.data, frame);
                assert_eq!(
                    sample.pts,
                    Timestamp::from_micros(index as u64 * frame_micros),
                    "{config:?}"
                );
            }
        }
    }

    #[test]
    fn a_changed_video_configuration_ends_the_file_and_starts_the_next() {
        // Arrange: a keyframe whose SPS declares another level
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(None), None).unwrap();
        let origin = None;
        let mut file = BytesMut::new();
        for object in &fixture.video[..3] {
            file.extend_from_slice(&pipeline.push_video(object, origin).unwrap().data);
        }
        let record = fixture
            .events
            .iter()
            .find_map(|event| match event {
                MediaEvent::VideoConfig(VideoConfig::Avc(record)) => Some(record.clone()),
                _ => None,
            })
            .unwrap();
        let mut sps = record.sps[0].to_vec();
        sps[3] = sps[3].wrapping_add(1);
        let changed = AvcDecoderConfigurationRecord::from_parameter_sets(
            vec![Bytes::from(sps)],
            record.pps.clone(),
        )
        .unwrap();
        let capture = first_video_capture(&fixture.video[2]) + 40_000;
        let keyframe = track_object(
            99,
            0,
            LocObject {
                extensions: vec![loc::LocExtension::varint(
                    loc::CAPTURE_TIMESTAMP_ID,
                    capture,
                )],
                payload: changed.with_parameter_sets(Bytes::from_static(IDR_SLICE)),
            },
        );

        // Act
        let rotated = pipeline.push_video(&keyframe, origin).unwrap();
        let next = pipeline
            .push_video(
                &track_object(
                    99,
                    1,
                    LocObject {
                        extensions: vec![loc::LocExtension::varint(
                            loc::CAPTURE_TIMESTAMP_ID,
                            capture + 40_000,
                        )],
                        payload: Bytes::from_static(&[0, 0, 0, 1, 0x41, 0x9a, 0x22]),
                    },
                ),
                origin,
            )
            .unwrap();

        // Assert: the first file holds the three samples, the next starts with its own init segment
        file.extend_from_slice(rotated.previous_file_tail.as_ref().unwrap());
        assert_eq!(video_samples(&demux(&file)).len(), 3);
        assert!(rotated.group_complete);
        let mut next_file = BytesMut::from(rotated.data.as_ref());
        next_file.extend_from_slice(&next.data);
        next_file.extend_from_slice(&pipeline.finish().unwrap());
        assert_eq!(&next_file[4..8], b"ftyp");
        let recorded = demux(&next_file);
        assert!(recorded.contains(&MediaEvent::VideoConfig(VideoConfig::Avc(changed))));
        assert_eq!(video_samples(&recorded).len(), 2);
    }

    #[test]
    fn an_audio_track_added_after_the_file_started_begins_with_the_next_keyframe() {
        // Arrange: the first group is written as video only
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(None), None).unwrap();
        let origin = capture_timestamp(&fixture.video[0]);
        let start = first_video_capture(&fixture.video[0]);
        let (first_group, later_groups): (Vec<_>, Vec<_>) = fixture
            .video
            .iter()
            .partition(|object| object.group_id == 1);
        let mut first_file = BytesMut::new();
        for object in first_group {
            first_file.extend_from_slice(&pipeline.push_video(object, origin).unwrap().data);
        }

        // Act
        pipeline.add_audio(opus_config()).unwrap();
        let before_keyframe = pipeline
            .push_audio(&opus_object(1, 0, start + 20_000), origin)
            .unwrap();
        let rotated = pipeline.push_video(later_groups[0], origin).unwrap();
        let after_keyframe = pipeline
            .push_audio(
                &opus_object(2, 0, first_video_capture(later_groups[0])),
                origin,
            )
            .unwrap();

        // Assert: the audio before the keyframe is dropped, the next file holds both tracks
        assert!(before_keyframe.data.is_empty());
        first_file.extend_from_slice(rotated.previous_file_tail.as_ref().unwrap());
        let mut next_file = BytesMut::from(rotated.data.as_ref());
        next_file.extend_from_slice(&after_keyframe.data);
        next_file.extend_from_slice(&pipeline.finish().unwrap());
        let first = demux(&first_file);
        let next = demux(&next_file);
        assert!(audio_samples(&first).is_empty());
        assert!(!first.contains(&MediaEvent::AudioConfig(opus_config())));
        assert!(next.contains(&MediaEvent::AudioConfig(opus_config())));
        assert_eq!(audio_samples(&next).len(), 1);
        assert!(video_samples(&next)[0].is_keyframe);
    }

    #[test]
    fn video_before_the_first_keyframe_is_dropped() {
        // Arrange: the recording joins the fixture's first group after its keyframe
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(None), None).unwrap();
        let origin = None;
        let second_keyframe = fixture
            .video
            .iter()
            .position(|object| object.group_id == 2)
            .unwrap();
        let mut file = BytesMut::new();

        // Act
        for object in &fixture.video[1..] {
            file.extend_from_slice(&pipeline.push_video(object, origin).unwrap().data);
        }
        file.extend_from_slice(&pipeline.finish().unwrap());

        // Assert
        let recorded = demux(&file);
        let video = video_samples(&recorded);
        assert_eq!(video.len(), fixture.video.len() - second_keyframe);
        assert!(video[0].is_keyframe);
    }

    #[test]
    fn an_audio_only_segment_ends_at_the_first_frame_past_its_length() {
        // Arrange
        let tracks = FileTracks {
            video: None,
            audio: Some(opus_config()),
        };
        let mut pipeline = Pipeline::new(tracks, Some(Duration::from_millis(100))).unwrap();
        let origin = None;
        let mut files = vec![BytesMut::new()];

        // Act
        for index in 0..10 {
            let object = opus_object(1, index, 1_000_000 + index * 20_000);
            let output = pipeline.push_audio(&object, origin).unwrap();
            if let Some(tail) = output.previous_file_tail {
                files.last_mut().unwrap().extend_from_slice(&tail);
                files.push(BytesMut::new());
            }
            files.last_mut().unwrap().extend_from_slice(&output.data);
        }

        // Assert: the second file starts at 100 ms, on the recording's timeline
        assert_eq!(files.len(), 2);
        let first = demux(&files[0]);
        let second = demux(&files[1]);
        assert!(video_samples(&first).is_empty());
        assert_eq!(audio_samples(&first).len(), 5);
        let second_audio = audio_samples(&second);
        assert_eq!(second_audio.len(), 5);
        assert_eq!(second_audio[0].pts, Timestamp::from_millis(100));
    }

    #[test]
    fn a_segment_ends_at_the_first_keyframe_past_its_length() {
        // Arrange: a 1 ms segment ends at the fixture's second keyframe
        let fixture = fixture();
        let mut pipeline = Pipeline::new(tracks(None), Some(Duration::from_millis(1))).unwrap();
        let origin = None;
        let second_keyframe = fixture
            .video
            .iter()
            .position(|object| object.group_id == 2)
            .unwrap();
        let mut files = vec![BytesMut::new()];

        // Act
        for object in &fixture.video {
            let output = pipeline.push_video(object, origin).unwrap();
            if let Some(tail) = output.previous_file_tail {
                files.last_mut().unwrap().extend_from_slice(&tail);
                files.push(BytesMut::new());
            }
            files.last_mut().unwrap().extend_from_slice(&output.data);
        }
        files
            .last_mut()
            .unwrap()
            .extend_from_slice(&pipeline.finish().unwrap());

        // Assert: each file starts with its own init segment and a keyframe
        assert_eq!(files.len(), 2);
        let first = demux(&files[0]);
        let second = demux(&files[1]);
        assert_eq!(video_samples(&first).len(), second_keyframe);
        assert_eq!(
            video_samples(&second).len(),
            fixture.video.len() - second_keyframe
        );
        assert!(video_samples(&second)[0].is_keyframe);
    }
}

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use mediapack::{
    AudioConfig, MediaEvent, StreamSet, Timestamp,
    h264::AvcDecoderConfigurationRecord,
    loc::{self, LocObject},
    mp4::Fmp4Muxer,
};
use moqt::TrackObject;

use crate::track_selection::Selection;

/// What one object produced. `previous_file_tail` ends the file written so
/// far when a configuration change forces a new init segment; `data` then
/// belongs to the next file.
pub(crate) struct Output {
    pub(crate) previous_file_tail: Option<Bytes>,
    pub(crate) data: Bytes,
    /// The video moved to a new group, so every earlier group is complete.
    pub(crate) group_complete: bool,
}

/// Both demuxers count presentation time from the first capture timestamp
/// either track delivers, so the file keeps the offset between the tracks.
/// Each demuxer is created at its track's first object, once that origin is
/// known.
pub(crate) struct Pipeline {
    video: Option<loc::Demuxer>,
    audio: Option<loc::Demuxer>,
    audio_track: Option<AudioConfig>,
    streams: StreamSet,
    muxer: Fmp4Muxer,
    capture_origin: Option<Timestamp>,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioConfig>,
    file_started: bool,
    current_group: Option<u64>,
}

impl Pipeline {
    pub(crate) fn new(selection: &Selection) -> Result<Self> {
        let streams = StreamSet {
            has_video: true,
            has_audio: selection.audio.is_some(),
        };
        let audio_config = selection.audio.as_ref().map(|audio| audio.config.clone());
        Ok(Self {
            video: None,
            audio: None,
            audio_track: audio_config.clone(),
            streams,
            muxer: new_muxer(streams, None, audio_config.as_ref())?,
            capture_origin: None,
            video_config: None,
            audio_config,
            file_started: false,
            current_group: None,
        })
    }

    pub(crate) fn push_video(&mut self, object: &TrackObject) -> Result<Output> {
        let group_complete = self
            .current_group
            .replace(object.group_id)
            .is_some_and(|previous| previous != object.group_id);
        let loc_object = to_loc_object(object);
        let origin = self.capture_origin(&loc_object);
        let events = demuxer(&mut self.video, origin, loc::Demuxer::video).push(&loc_object)?;
        let mut output = self.mux(events)?;
        output.group_complete = group_complete;
        Ok(output)
    }

    pub(crate) fn push_audio(&mut self, object: &TrackObject) -> Result<Output> {
        let config = self
            .audio_track
            .clone()
            .context("audio object without an audio track")?;
        let loc_object = to_loc_object(object);
        let origin = self.capture_origin(&loc_object);
        let events =
            demuxer(&mut self.audio, origin, || loc::Demuxer::audio(config)).push(&loc_object)?;
        self.mux(events)
    }

    /// The fragment of the last video sample, whose duration was unknown
    /// until now.
    pub(crate) fn finish(&mut self) -> Result<Bytes> {
        self.muxer.finish()
    }

    fn capture_origin(&mut self, object: &LocObject) -> Option<Timestamp> {
        if let Some(capture) = object.capture_timestamp() {
            self.capture_origin.get_or_insert(capture);
        }
        self.capture_origin
    }

    /// The demuxers' own `Streams` events are dropped: each announces its one
    /// track, while the muxer was told about both when the pipeline started.
    fn mux(&mut self, events: Vec<MediaEvent>) -> Result<Output> {
        let mut tail: Option<BytesMut> = None;
        let mut data = BytesMut::new();
        for event in events {
            let changed = match &event {
                MediaEvent::Streams(_) => continue,
                MediaEvent::VideoConfig(config) => self.video_config.as_ref() != Some(config),
                MediaEvent::AudioConfig(config) => self.audio_config.as_ref() != Some(config),
                _ => false,
            };
            if changed && self.file_started {
                tail.get_or_insert_default()
                    .extend_from_slice(&self.rotate(&mut data)?);
            }
            match &event {
                MediaEvent::VideoConfig(config) => self.video_config = Some(config.clone()),
                MediaEvent::AudioConfig(config) => self.audio_config = Some(config.clone()),
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
        self.file_started = false;
        Ok(tail.freeze())
    }
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
    video_config: Option<&AvcDecoderConfigurationRecord>,
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
    use mediapack::{AudioSample, VideoSample, mp4, mpegts};

    use super::*;
    use crate::track_selection::AudioTrack;

    const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");
    const CAPTURE_ORIGIN: Timestamp = Timestamp::from_micros(1_700_000_000_000_000);
    /// An IDR slice NAL in Annex-B form; only its NAL type is read.
    const IDR_SLICE: &[u8] = &[0, 0, 0, 1, 0x65, 0x88, 0x84];

    /// The fixture's media events as LOC track objects, grouped per keyframe
    /// the way `broadcast` numbers them, plus the AAC config the catalog
    /// would carry.
    struct Fixture {
        events: Vec<MediaEvent>,
        video: Vec<TrackObject>,
        audio: Vec<TrackObject>,
        audio_config: AudioConfig,
    }

    fn fixture() -> Fixture {
        let mut demuxer = mpegts::Demuxer::new();
        let mut events = demuxer.push(FIXTURE_TS).unwrap();
        events.extend(demuxer.finish().unwrap());
        let muxer = loc::Muxer::new(CAPTURE_ORIGIN);
        let (mut video, mut audio) = (Vec::new(), Vec::new());
        let mut group_id = 0;
        for event in &events {
            let Some(object) = muxer.push(event) else {
                continue;
            };
            match event {
                MediaEvent::Video(sample) => {
                    if sample.is_keyframe {
                        group_id += 1;
                    }
                    video.push(track_object(group_id, video.len() as u64, object));
                }
                MediaEvent::Audio(_) => {
                    audio.push(track_object(group_id, audio.len() as u64, object))
                }
                _ => {}
            }
        }
        let audio_config = events
            .iter()
            .find_map(|event| match event {
                MediaEvent::AudioConfig(config) => Some(config.clone()),
                _ => None,
            })
            .unwrap();
        Fixture {
            events,
            video,
            audio,
            audio_config,
        }
    }

    fn track_object(group_id: u64, object_id: u64, object: LocObject) -> TrackObject {
        TrackObject {
            group_id,
            subgroup_id: 0,
            object_id,
            extension_headers: loc::to_extension_headers(&object.extensions),
            payload: object.payload,
        }
    }

    fn selection(audio_config: Option<AudioConfig>) -> Selection {
        Selection {
            video: "video".to_string(),
            audio: audio_config.map(|config| AudioTrack {
                name: "audio".to_string(),
                config,
            }),
        }
    }

    fn demux(data: &[u8]) -> Vec<MediaEvent> {
        let mut demuxer = mp4::Demuxer::new();
        let mut events = demuxer.push(data).unwrap();
        events.extend(demuxer.finish().unwrap());
        events
    }

    fn video_samples(events: &[MediaEvent]) -> Vec<&VideoSample> {
        events
            .iter()
            .filter_map(|event| match event {
                MediaEvent::Video(sample) => Some(sample),
                _ => None,
            })
            .collect()
    }

    fn audio_samples(events: &[MediaEvent]) -> Vec<&AudioSample> {
        events
            .iter()
            .filter_map(|event| match event {
                MediaEvent::Audio(sample) => Some(sample),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn writes_every_sample_of_both_tracks_into_one_file() {
        // Arrange: the audio of a group follows the first video object of the group
        let fixture = fixture();
        let mut pipeline = Pipeline::new(&selection(Some(fixture.audio_config.clone()))).unwrap();
        let mut file = BytesMut::new();
        let mut groups_completed = 0;
        let mut audio = fixture.audio.iter().peekable();

        // Act
        for object in &fixture.video {
            let output = pipeline.push_video(object).unwrap();
            assert_eq!(output.previous_file_tail, None);
            groups_completed += output.group_complete as usize;
            file.extend_from_slice(&output.data);
            while let Some(next) = audio.next_if(|audio| audio.group_id <= object.group_id) {
                file.extend_from_slice(&pipeline.push_audio(next).unwrap().data);
            }
        }
        for next in audio {
            file.extend_from_slice(&pipeline.push_audio(next).unwrap().data);
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
        let mut pipeline = Pipeline::new(&selection(Some(fixture.audio_config.clone()))).unwrap();
        let first_video = fixture.video[0].clone();
        let mut late_audio = fixture.audio[0].clone();
        let video_capture = first_video_capture(&first_video);
        late_audio.extension_headers = loc::to_extension_headers(&[loc::LocExtension::varint(
            loc::CAPTURE_TIMESTAMP_ID,
            video_capture + 30_000,
        )]);
        let mut file = BytesMut::new();

        // Act
        file.extend_from_slice(&pipeline.push_video(&first_video).unwrap().data);
        file.extend_from_slice(&pipeline.push_audio(&late_audio).unwrap().data);
        file.extend_from_slice(&pipeline.push_video(&fixture.video[1]).unwrap().data);
        file.extend_from_slice(&pipeline.finish().unwrap());

        // Assert
        let recorded = demux(&file);
        let video = video_samples(&recorded);
        let audio = audio_samples(&recorded);
        assert_eq!(video[0].pts, Timestamp::ZERO);
        assert!(audio[0].pts.micros().abs_diff(30_000) <= 21);
    }

    #[test]
    fn a_changed_video_configuration_ends_the_file_and_starts_the_next() {
        // Arrange: a keyframe whose SPS declares another level
        let fixture = fixture();
        let mut pipeline = Pipeline::new(&selection(None)).unwrap();
        let mut file = BytesMut::new();
        for object in &fixture.video[..3] {
            file.extend_from_slice(&pipeline.push_video(object).unwrap().data);
        }
        let record = fixture
            .events
            .iter()
            .find_map(|event| match event {
                MediaEvent::VideoConfig(config) => Some(config.clone()),
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
        let rotated = pipeline.push_video(&keyframe).unwrap();
        let next = pipeline
            .push_video(&track_object(
                99,
                1,
                LocObject {
                    extensions: vec![loc::LocExtension::varint(
                        loc::CAPTURE_TIMESTAMP_ID,
                        capture + 40_000,
                    )],
                    payload: Bytes::from_static(&[0, 0, 0, 1, 0x41, 0x9a, 0x22]),
                },
            ))
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
        assert!(recorded.contains(&MediaEvent::VideoConfig(changed)));
        assert_eq!(video_samples(&recorded).len(), 2);
    }

    fn first_video_capture(object: &TrackObject) -> u64 {
        to_loc_object(object).capture_timestamp().unwrap().micros()
    }
}

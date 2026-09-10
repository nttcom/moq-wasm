use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::{
    AudioSample, MediaEvent, Timestamp, VideoSample, aac::AudioSpecificConfig,
    loc::Muxer as LocMuxer, mp4::Fmp4TrackMuxer,
};
use moqt::ExtensionHeaders;

use crate::{
    group_alignment::GroupAlignment,
    loc_object::extension_headers,
    media_timeline::MediaTimeline,
    moqt::{
        GroupBoundary, MoqtManager, OutgoingObject, TIMELINE_TRACK_NAME, VIDEO_TRACK_NAME,
        VideoTrackInfo, cmaf_track_name, now_unix,
    },
    renditions::RenditionFanout,
};

const AUDIO_GROUP_ROTATION_INTERVAL_US: u64 = 2_000_000;
const AUDIO_TRACK: &str = "audio";

#[derive(Clone)]
pub struct IngestOptions {
    pub moqt_url: Option<String>,
    pub transcode: bool,
}

pub struct MediaPublisher {
    moqt: MoqtManager,
    namespace: Vec<String>,
    transcode: bool,
    namespace_ready: bool,
    audio_group_duration_us: u64,
    audio_config: Option<AudioSpecificConfig>,
    renditions: Option<RenditionFanout>,
    timeline: MediaTimeline,
    /// The LOC capture timestamp is wall-clock time of the sample, so the muxer
    /// is seeded with the wall-clock time of presentation time zero once the
    /// first sample arrives. Renditions share it so every track stamps the same
    /// instant for the same presentation time.
    loc: Arc<OnceLock<LocMuxer>>,
    alignment: Arc<GroupAlignment>,
    video_cmaf: Option<Fmp4TrackMuxer>,
    audio_cmaf: Option<Fmp4TrackMuxer>,
}

impl MediaPublisher {
    pub fn new(moqt: MoqtManager, namespace: Vec<String>, transcode: bool) -> Self {
        Self {
            moqt,
            namespace,
            transcode,
            namespace_ready: false,
            audio_group_duration_us: 0,
            audio_config: None,
            renditions: None,
            timeline: MediaTimeline::new(),
            loc: Arc::new(OnceLock::new()),
            alignment: Arc::new(GroupAlignment::new(wall_clock().micros())),
            video_cmaf: None,
            audio_cmaf: None,
        }
    }

    pub async fn push(&mut self, event: &MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Streams(_) => Ok(()),
            MediaEvent::VideoConfig(config) => {
                self.video_cmaf = Some(Fmp4TrackMuxer::video(config.clone()));
                let info = VideoTrackInfo::from_record(config, "Video".to_string())?;
                if self.transcode && self.renditions.is_none() {
                    self.renditions = RenditionFanout::run(
                        self.moqt.clone(),
                        self.namespace.clone(),
                        &info,
                        self.loc.clone(),
                        self.alignment.clone(),
                    )?;
                }
                self.moqt
                    .update_video_catalog(&self.namespace, VIDEO_TRACK_NAME, info)
                    .await
            }
            MediaEvent::AudioConfig(config) => {
                self.audio_cmaf = Some(Fmp4TrackMuxer::audio(config.clone()));
                self.moqt
                    .update_audio_catalog(&self.namespace, config.clone())
                    .await?;
                self.audio_config = Some(config.clone());
                Ok(())
            }
            MediaEvent::Video(sample) => self.publish_video(sample).await,
            MediaEvent::Audio(sample) => self.publish_audio(sample).await,
        }
    }

    async fn publish_video(&mut self, sample: &VideoSample) -> Result<()> {
        self.setup_namespace().await?;
        if let Some(renditions) = &self.renditions {
            renditions.push(sample);
        }
        let Some(object) = self
            .loc_muxer(sample.pts)
            .push(&MediaEvent::Video(sample.clone()))
        else {
            return Ok(());
        };
        let captured_at = object.capture_timestamp();
        let keyframe_group = sample
            .is_keyframe
            .then(|| self.alignment.keyframe(sample.pts.micros()));
        self.moqt
            .send_object(
                &self.namespace,
                VIDEO_TRACK_NAME,
                OutgoingObject {
                    group: keyframe_group.map_or(GroupBoundary::Within, GroupBoundary::At),
                    extension_headers: extension_headers(&object),
                    payload: object.payload,
                },
            )
            .await?;
        self.publish_cmaf_video(sample).await?;
        let (Some(group_id), Some(captured_at)) = (keyframe_group, captured_at) else {
            return Ok(());
        };
        self.publish_timeline(group_id, sample.pts.micros(), captured_at.millis())
            .await
    }

    async fn publish_cmaf_video(&mut self, sample: &VideoSample) -> Result<()> {
        let Some(muxer) = &mut self.video_cmaf else {
            return Ok(());
        };
        let Some(fragment) = muxer.push(&MediaEvent::Video(sample.clone()))? else {
            return Ok(());
        };
        let group = if fragment.is_keyframe {
            GroupBoundary::At(self.alignment.keyframe(fragment.presentation_time.micros()))
        } else {
            GroupBoundary::Within
        };
        self.moqt
            .send_object(
                &self.namespace,
                &cmaf_track_name(VIDEO_TRACK_NAME),
                OutgoingObject {
                    group,
                    extension_headers: ExtensionHeaders::default(),
                    payload: fragment.data,
                },
            )
            .await
    }

    async fn publish_timeline(
        &mut self,
        group_id: u64,
        presentation_us: u64,
        encoded_at_ms: u64,
    ) -> Result<()> {
        self.timeline
            .record(group_id, presentation_us, encoded_at_ms);
        self.moqt
            .send_object(
                &self.namespace,
                TIMELINE_TRACK_NAME,
                OutgoingObject {
                    group: GroupBoundary::Next,
                    extension_headers: ExtensionHeaders::default(),
                    payload: Bytes::from(self.timeline.document()?),
                },
            )
            .await
    }

    async fn publish_audio(&mut self, sample: &AudioSample) -> Result<()> {
        let frame_duration_us = self
            .audio_config
            .as_ref()
            .context("audio sample received before its AudioSpecificConfig")?
            .frame_duration()
            .micros();
        self.setup_namespace().await?;
        let group = if self.rotate_audio_group(frame_duration_us) {
            GroupBoundary::Next
        } else {
            GroupBoundary::Join
        };
        let Some(object) = self
            .loc_muxer(sample.pts)
            .push(&MediaEvent::Audio(sample.clone()))
        else {
            return Ok(());
        };
        self.moqt
            .send_object(
                &self.namespace,
                AUDIO_TRACK,
                OutgoingObject {
                    group,
                    extension_headers: extension_headers(&object),
                    payload: object.payload,
                },
            )
            .await?;
        self.publish_cmaf_audio(sample, group).await
    }

    async fn publish_cmaf_audio(
        &mut self,
        sample: &AudioSample,
        group: GroupBoundary,
    ) -> Result<()> {
        let Some(muxer) = &mut self.audio_cmaf else {
            return Ok(());
        };
        let Some(fragment) = muxer.push(&MediaEvent::Audio(sample.clone()))? else {
            return Ok(());
        };
        self.moqt
            .send_object(
                &self.namespace,
                &cmaf_track_name(AUDIO_TRACK),
                OutgoingObject {
                    group,
                    extension_headers: ExtensionHeaders::default(),
                    payload: fragment.data,
                },
            )
            .await
    }

    fn loc_muxer(&self, presentation_time: Timestamp) -> &LocMuxer {
        self.loc
            .get_or_init(|| LocMuxer::new(wall_clock().saturating_sub(presentation_time)))
    }

    async fn setup_namespace(&mut self) -> Result<()> {
        if !self.namespace_ready {
            self.moqt.setup_namespace(&self.namespace).await?;
            self.namespace_ready = true;
        }
        Ok(())
    }

    fn rotate_audio_group(&mut self, duration_us: u64) -> bool {
        let rotate = self.audio_group_duration_us != 0
            && self.audio_group_duration_us.saturating_add(duration_us)
                > AUDIO_GROUP_ROTATION_INTERVAL_US;
        self.audio_group_duration_us = if rotate {
            duration_us
        } else {
            self.audio_group_duration_us.saturating_add(duration_us)
        };
        rotate
    }
}

fn wall_clock() -> Timestamp {
    Timestamp::from_micros(now_unix().as_micros() as u64)
}

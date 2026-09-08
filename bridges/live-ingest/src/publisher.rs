use anyhow::{Context, Result};
use mediapack::{AudioSample, MediaEvent, VideoSample, aac::AudioSpecificConfig};

use crate::{
    chunk_payload::{pack_audio_chunk_payload, pack_video_chunk_payload},
    moqt::{MoqtManager, VIDEO_TRACK_NAME, VideoTrackInfo, now_unix},
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
    video_codec: Option<String>,
    audio_config: Option<AudioSpecificConfig>,
    renditions: Option<RenditionFanout>,
}

impl MediaPublisher {
    pub fn new(moqt: MoqtManager, namespace: Vec<String>, transcode: bool) -> Self {
        Self {
            moqt,
            namespace,
            transcode,
            namespace_ready: false,
            audio_group_duration_us: 0,
            video_codec: None,
            audio_config: None,
            renditions: None,
        }
    }

    pub async fn push(&mut self, event: &MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Streams(_) => Ok(()),
            MediaEvent::VideoConfig(config) => {
                let info = VideoTrackInfo::from_record(config, "Video".to_string())?;
                self.video_codec = Some(info.codec.clone());
                if self.transcode && self.renditions.is_none() {
                    self.renditions =
                        RenditionFanout::run(self.moqt.clone(), self.namespace.clone(), &info)?;
                }
                self.moqt
                    .update_video_catalog(&self.namespace, VIDEO_TRACK_NAME, info)
                    .await
            }
            MediaEvent::AudioConfig(config) => {
                self.moqt
                    .update_audio_catalog(
                        &self.namespace,
                        config.sample_rate,
                        config.channel_count(),
                    )
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
        let payload = video_payload(sample, self.video_codec.as_deref());
        self.moqt
            .send_object(
                &self.namespace,
                VIDEO_TRACK_NAME,
                sample.is_keyframe,
                payload,
            )
            .await?;
        if let Some(renditions) = &self.renditions {
            renditions.push(sample);
        }
        Ok(())
    }

    async fn publish_audio(&mut self, sample: &AudioSample) -> Result<()> {
        let config = self
            .audio_config
            .clone()
            .context("audio sample received before its AudioSpecificConfig")?;
        self.setup_namespace().await?;
        let duration_us = config.frame_duration().micros();
        let rotate_group = self.rotate_audio_group(duration_us);
        let payload = pack_audio_chunk_payload(
            &sample.data,
            &config,
            sample.pts.micros(),
            duration_us,
            now_unix().as_millis() as u64,
        );
        self.moqt
            .send_object(&self.namespace, AUDIO_TRACK, rotate_group, payload)
            .await
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

pub fn video_payload(sample: &VideoSample, codec: Option<&str>) -> Vec<u8> {
    pack_video_chunk_payload(
        sample.is_keyframe,
        sample.pts.micros(),
        now_unix().as_millis() as u64,
        &sample.data,
        codec.filter(|_| sample.is_keyframe),
    )
}

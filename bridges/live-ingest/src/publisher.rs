use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Error, Result};
use mediapack::{
    AudioSample, MediaEvent, VideoSample, aac::AudioSpecificConfig,
    h264::AvcDecoderConfigurationRecord,
};

use crate::{
    chunk_payload::{pack_audio_chunk_payload, pack_video_chunk_payload},
    moqt::MoqtManager,
};

const AUDIO_GROUP_ROTATION_INTERVAL_US: u64 = 2_000_000;
const VIDEO_TRACK: &str = "video";
const AUDIO_TRACK: &str = "audio";

pub struct MediaPublisher {
    moqt: MoqtManager,
    namespace: Vec<String>,
    namespace_ready: bool,
    audio_group_duration_us: u64,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioSpecificConfig>,
}

impl MediaPublisher {
    pub fn new(moqt: MoqtManager, namespace: Vec<String>) -> Self {
        Self {
            moqt,
            namespace,
            namespace_ready: false,
            audio_group_duration_us: 0,
            video_config: None,
            audio_config: None,
        }
    }

    pub async fn push(&mut self, event: &MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Streams(_) => Ok(()),
            MediaEvent::VideoConfig(config) => {
                self.moqt
                    .update_video_catalog(&self.namespace, Some(&config.codec_string()))
                    .await?;
                self.video_config = Some(config.clone());
                Ok(())
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
        let codec = self
            .video_config
            .as_ref()
            .filter(|_| sample.is_keyframe)
            .map(|config| config.codec_string());
        let payload = pack_video_chunk_payload(
            sample.is_keyframe,
            sample.pts.micros(),
            current_time_ms(),
            &sample.data,
            codec.as_deref(),
            None,
        );
        self.send(VIDEO_TRACK, sample.is_keyframe, &payload).await
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
            current_time_ms(),
        );
        self.send(AUDIO_TRACK, rotate_group, &payload).await
    }

    async fn setup_namespace(&mut self) -> Result<()> {
        if !self.namespace_ready {
            self.moqt.setup_namespace(&self.namespace).await?;
            self.namespace_ready = true;
        }
        Ok(())
    }

    async fn send(&self, track: &str, rotate_group: bool, payload: &[u8]) -> Result<()> {
        match self
            .moqt
            .send_object(&self.namespace, track, rotate_group, payload)
            .await
        {
            Err(error) if is_expected_pre_subscribe_send_error(&error) => Ok(()),
            result => result,
        }
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

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_expected_pre_subscribe_send_error(error: &Error) -> bool {
    let message = error.to_string();
    message.contains("track not set up:") || message.contains("subscribe not completed:")
}

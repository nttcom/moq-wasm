use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Error, Result};

use crate::{
    audio::{AudioFrame, compute_aac_duration_us, pack_audio_chunk_payload},
    moqt::MoqtManager,
    video::{VideoFrame, pack_video_chunk_payload},
};

const AUDIO_GROUP_ROTATION_INTERVAL_US: u64 = 2_000_000;

pub struct MediaPublisher {
    moqt: MoqtManager,
    namespace: Vec<String>,
    namespace_ready: bool,
    audio_group_duration_us: u64,
}

impl MediaPublisher {
    pub fn new(moqt: MoqtManager, namespace: Vec<String>) -> Self {
        Self {
            moqt,
            namespace,
            namespace_ready: false,
            audio_group_duration_us: 0,
        }
    }

    pub async fn publish_audio(&mut self, frame: AudioFrame, timestamp_us: u64) -> Result<()> {
        let duration_us = compute_aac_duration_us(frame.sample_rate);
        self.moqt
            .update_audio_catalog(&self.namespace, frame.sample_rate, frame.channels)
            .await?;
        self.setup_namespace().await?;

        let rotate_group = self.rotate_audio_group(duration_us);
        let payload =
            pack_audio_chunk_payload(&frame, timestamp_us, Some(duration_us), current_time_ms());
        self.send("audio", rotate_group, &payload).await
    }

    pub async fn publish_video(&mut self, frame: VideoFrame, timestamp_us: u64) -> Result<()> {
        if let Some(codec) = frame.codec.as_deref() {
            self.moqt
                .update_video_catalog(&self.namespace, Some(codec))
                .await?;
        }
        self.setup_namespace().await?;

        let payload = pack_video_chunk_payload(
            frame.is_key,
            timestamp_us,
            current_time_ms(),
            &frame.data,
            frame.codec.as_deref(),
            None,
        );
        self.send("video", frame.is_key, &payload).await
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

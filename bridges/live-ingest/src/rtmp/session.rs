use std::collections::{HashMap, VecDeque};

use anyhow::Result;
use bytes::Bytes;
use mediapack::{
    MediaEvent, Timestamp,
    flv::{self, Tag, TagType},
};
use rml_rtmp::sessions::{ServerSession, ServerSessionEvent, ServerSessionResult};

use crate::{ingest::flv::FlvRecorder, moqt::MoqtManager, publisher::MediaPublisher};

#[derive(Default)]
pub struct RtmpCounters {
    pub audio: u64,
    pub video: u64,
}

pub struct RtmpState {
    pub counters: RtmpCounters,
    pub recorder: Option<FlvRecorder>,
    pub moqt: MoqtManager,
    pub streams: HashMap<String, RtmpStream>,
}

pub struct RtmpStream {
    demuxer: flv::Demuxer,
    publisher: MediaPublisher,
}

impl RtmpState {
    pub fn new(moqt: MoqtManager) -> Self {
        Self {
            counters: RtmpCounters::default(),
            recorder: None,
            moqt,
            streams: HashMap::new(),
        }
    }
}

pub async fn handle_event(
    session: &mut ServerSession,
    queue: &mut VecDeque<ServerSessionResult>,
    event: ServerSessionEvent,
    label: &str,
    state: &mut RtmpState,
) -> Result<()> {
    match event {
        ServerSessionEvent::ConnectionRequested {
            request_id,
            app_name,
        } => {
            tracing::info!(peer = %label, app = %app_name, "RTMP connect requested");
            queue.extend(session.accept_request(request_id)?);
        }
        ServerSessionEvent::PublishStreamRequested {
            request_id,
            app_name,
            stream_key,
            mode,
        } => {
            let (namespace_path, cleaned_stream_key) =
                split_namespace_and_key(&app_name, &stream_key);
            tracing::info!(peer = %label, app = %app_name, stream = %cleaned_stream_key, namespace = %namespace_path, ?mode, "RTMP publish requested");
            queue.extend(session.accept_request(request_id)?);
            if state.recorder.is_none() {
                match FlvRecorder::open(&app_name, &stream_key).await {
                    Ok(recorder) => state.recorder = Some(recorder),
                    Err(err) => {
                        tracing::warn!(peer = %label, ?err, "failed to start FLV recorder");
                    }
                }
            }
        }
        ServerSessionEvent::PublishStreamFinished {
            app_name,
            stream_key,
        } => {
            tracing::info!(peer = %label, app = %app_name, stream = %stream_key, "RTMP publish finished");
            let (namespace_path, _cleaned_stream_key) =
                split_namespace_and_key(&app_name, &stream_key);
            state.streams.remove(&namespace_path);
        }
        ServerSessionEvent::StreamMetadataChanged {
            app_name,
            stream_key,
            metadata: _,
        } => {
            tracing::debug!(peer = %label, app = %app_name, stream = %stream_key, "RTMP metadata updated");
        }
        ServerSessionEvent::AudioDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            state.counters.audio += 1;
            if state.counters.audio == 1 || state.counters.audio.is_multiple_of(1000) {
                tracing::debug!(peer = %label, app = %app_name, packets = state.counters.audio, "RTMP audio packets received");
            }
            let tag = Tag {
                tag_type: TagType::Audio,
                timestamp: Timestamp::from_millis(timestamp.value as u64),
                data: Bytes::copy_from_slice(&data),
            };
            if let Some(recorder) = state.recorder.as_mut()
                && let Err(err) = recorder.write_tag(&tag).await
            {
                tracing::warn!(peer = %label, ?err, "failed to record RTMP audio tag");
            }
            handle_media_tag(state, label, &app_name, &stream_key, &tag).await;
        }
        ServerSessionEvent::VideoDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            state.counters.video += 1;
            if state.counters.video == 1 || state.counters.video.is_multiple_of(1000) {
                tracing::debug!(peer = %label, app = %app_name, packets = state.counters.video, "RTMP video packets received");
            }
            let tag = Tag {
                tag_type: TagType::Video,
                timestamp: Timestamp::from_millis(timestamp.value as u64),
                data: Bytes::copy_from_slice(&data),
            };
            if let Some(recorder) = state.recorder.as_mut()
                && let Err(err) = recorder.write_tag(&tag).await
            {
                tracing::warn!(peer = %label, ?err, "failed to record RTMP video tag");
            }
            handle_media_tag(state, label, &app_name, &stream_key, &tag).await;
        }
        ServerSessionEvent::PlayStreamRequested { request_id, .. } => {
            queue.extend(session.reject_request(
                request_id,
                "NetStream.Play.Failed",
                "playback not supported",
            )?);
        }
        other => {
            tracing::debug!(peer = %label, event = ?other, "unhandled RTMP event");
        }
    }

    Ok(())
}

async fn handle_media_tag(
    state: &mut RtmpState,
    label: &str,
    app_name: &str,
    stream_key: &str,
    tag: &Tag,
) {
    let (namespace_path, _stream_key) = split_namespace_and_key(app_name, stream_key);
    let stream = state
        .streams
        .entry(namespace_path.clone())
        .or_insert_with(|| RtmpStream {
            demuxer: flv::Demuxer::new(),
            publisher: MediaPublisher::new(
                state.moqt.clone(),
                namespace_path.split('/').map(str::to_owned).collect(),
            ),
        });
    let events = match stream.demuxer.push_tag(tag) {
        Ok(events) => events,
        Err(err) => {
            tracing::warn!(peer = %label, ?err, "failed to parse RTMP media tag");
            return;
        }
    };
    for event in &events {
        if let MediaEvent::VideoConfig(config) = event {
            tracing::info!(peer = %label, namespace = %namespace_path, codec = %config.codec_string(), "detected video codec");
        }
        if let Err(err) = stream.publisher.push(event).await {
            tracing::warn!(peer = %label, namespace = %namespace_path, ?err, "failed to publish RTMP media");
        }
    }
}

fn split_namespace_and_key(app_name: &str, stream_key: &str) -> (String, String) {
    let mut parts: Vec<&str> = stream_key.split('/').collect();
    if parts.len() >= 2 {
        let last = parts.pop().unwrap_or_default();
        let ns_suffix = parts.join("/");
        (format!("{app_name}/{ns_suffix}"), last.to_string())
    } else {
        (app_name.to_string(), stream_key.to_string())
    }
}

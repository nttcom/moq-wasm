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
            println!("[rtmp {label}] connect app={app_name}");
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
            println!(
                "[rtmp {label}] publish app={app_name} stream={cleaned_stream_key} -> ns={namespace_path} mode={mode:?}"
            );
            queue.extend(session.accept_request(request_id)?);
            if state.recorder.is_none() {
                match FlvRecorder::open(&app_name, &stream_key).await {
                    Ok(recorder) => state.recorder = Some(recorder),
                    Err(err) => {
                        eprintln!("[rtmp {label}] fail to start FLV recorder: {err:?}");
                    }
                }
            }
        }
        ServerSessionEvent::PublishStreamFinished {
            app_name,
            stream_key,
        } => {
            println!("[rtmp {label}] publish finished app={app_name} stream={stream_key}");
            let (namespace_path, _cleaned_stream_key) =
                split_namespace_and_key(&app_name, &stream_key);
            state.streams.remove(&namespace_path);
        }
        ServerSessionEvent::StreamMetadataChanged {
            app_name,
            stream_key,
            metadata: _,
        } => {
            println!("[rtmp {label}] metadata updated app={app_name} stream={stream_key}");
        }
        ServerSessionEvent::AudioDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            state.counters.audio += 1;
            if state.counters.audio == 1 || state.counters.audio.is_multiple_of(1000) {
                println!(
                    "[rtmp {label}] audio packets={} app={app_name}",
                    state.counters.audio
                );
            }
            let tag = Tag {
                tag_type: TagType::Audio,
                timestamp: Timestamp::from_millis(timestamp.value as u64),
                data: Bytes::copy_from_slice(&data),
            };
            if let Some(recorder) = state.recorder.as_mut()
                && let Err(err) = recorder.write_tag(&tag).await
            {
                eprintln!("[rtmp {label}] failed to record audio tag: {err:?}");
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
                println!(
                    "[rtmp {label}] video packets={} app={app_name}",
                    state.counters.video
                );
            }
            let tag = Tag {
                tag_type: TagType::Video,
                timestamp: Timestamp::from_millis(timestamp.value as u64),
                data: Bytes::copy_from_slice(&data),
            };
            if let Some(recorder) = state.recorder.as_mut()
                && let Err(err) = recorder.write_tag(&tag).await
            {
                eprintln!("[rtmp {label}] failed to record video tag: {err:?}");
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
            println!("[rtmp {label}] event {:?}", other);
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
            eprintln!("[rtmp {label}] flv parse failed: {err:?}");
            return;
        }
    };
    for event in &events {
        if let MediaEvent::VideoConfig(config) = event {
            println!(
                "[rtmp {label}] detected video codec: {} ns={namespace_path}",
                config.codec_string()
            );
        }
        if let Err(err) = stream.publisher.push(event).await {
            eprintln!("[rtmp {label}] moqt publish failed: {err:?}");
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

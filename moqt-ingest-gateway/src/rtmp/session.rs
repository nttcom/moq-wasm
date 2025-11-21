use std::collections::{HashMap, VecDeque};

use anyhow::Result;
use rml_rtmp::sessions::{ServerSession, ServerSessionEvent, ServerSessionResult};

use crate::{
    ingest::flv::FlvRecorder,
    moqt::MoqtManager,
    video::{pack_video_chunk_payload, AvcState},
};

#[derive(Default)]
pub struct RtmpCounters {
    pub audio: u64,
    pub video: u64,
}

pub struct RtmpState {
    pub counters: RtmpCounters,
    pub recorder: Option<FlvRecorder>,
    pub moqt: Option<MoqtManager>,
    pub video_states: HashMap<(String, String), AvcState>,
}

impl RtmpState {
    pub fn new(moqt: MoqtManager) -> Self {
        Self {
            counters: RtmpCounters::default(),
            recorder: None,
            moqt: Some(moqt),
            video_states: HashMap::new(),
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
            let namespace_path = app_name.clone(); // track_namespace は application 名
            println!(
                "[rtmp {label}] publish app={app_name} stream={stream_key} -> ns={namespace_path} mode={mode:?}"
            );
            queue.extend(session.accept_request(request_id)?);
            if state.recorder.is_none() {
                match FlvRecorder::spawn(&app_name, &stream_key).await {
                    Ok(recorder) => state.recorder = Some(recorder),
                    Err(err) => {
                        eprintln!("[rtmp {label}] fail to start ffmpeg recorder: {err:?}");
                    }
                }
            }
            if let Some(moqt) = state.moqt.as_ref() {
                let namespace = vec![namespace_path];
                if let Err(err) = moqt.setup_namespace(&namespace).await {
                    eprintln!("[rtmp {label}] moqt setup failed: {err:?}");
                }
            }
        }
        ServerSessionEvent::PublishStreamFinished {
            app_name,
            stream_key,
        } => {
            println!("[rtmp {label}] publish finished app={app_name} stream={stream_key}");
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
            if state.counters.audio == 1 || state.counters.audio % 1000 == 0 {
                println!(
                    "[rtmp {label}] audio packets={} app={app_name} stream={stream_key}",
                    state.counters.audio
                );
            }
            if let Some(recorder) = state.recorder.as_mut() {
                if let Err(err) = recorder.write_audio(timestamp.value, data.as_ref()).await {
                    eprintln!("[rtmp {label}] write_audio failed: {err:?}");
                }
            }
            // audio は現状 MoQ 送信しない
        }
        ServerSessionEvent::VideoDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            let track_name = "video".to_string();
            state.counters.video += 1;
            if state.counters.video == 1 || state.counters.video % 1000 == 0 {
                println!(
                    "[rtmp {label}] video packets={} app={app_name} track={track_name}",
                    state.counters.video
                );
            }
            if let Some(recorder) = state.recorder.as_mut() {
                if let Err(err) = recorder.write_video(timestamp.value, data.as_ref()).await {
                    eprintln!("[rtmp {label}] write_video failed: {err:?}");
                }
            }
            if let Some(moqt) = state.moqt.as_ref() {
                let namespace_path = app_name.clone(); // application 名をそのまま namespace に
                let namespace = [namespace_path.clone()];
                let key = (namespace_path, track_name.clone());
                let frame = match state
                    .video_states
                    .entry(key)
                    .or_default()
                    .handle_flv_video(data.as_ref())
                {
                    Ok(f) => f,
                    Err(err) => {
                        eprintln!("[rtmp {label}] h264 parse failed: {err:?}");
                        None
                    }
                };
                if let Some(frame) = frame {
                    // RTMP timestamp は ms 単位なので μs へ拡張
                    let timestamp_us = timestamp.value as u64 * 1_000;
                    let payload = pack_video_chunk_payload(
                        frame.is_key,
                        timestamp_us,
                        frame.data.as_slice(),
                    );
                    if let Err(err) =
                        moqt.send_object(&namespace, &track_name, frame.is_key, payload.as_slice()).await
                    {
                        eprintln!("[rtmp {label}] moqt send video failed: {err:?}");
                    }
                }
            }
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

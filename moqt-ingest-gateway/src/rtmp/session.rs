use std::collections::VecDeque;

use anyhow::Result;
use rml_rtmp::sessions::{ServerSession, ServerSessionEvent, ServerSessionResult};

use crate::{ingest::flv::FlvRecorder, moqt::MoqtManager};

#[derive(Default)]
pub struct RtmpCounters {
    pub audio: u64,
    pub video: u64,
}

pub struct RtmpState {
    pub counters: RtmpCounters,
    pub recorder: Option<FlvRecorder>,
    pub moqt: Option<MoqtManager>,
}

impl RtmpState {
    pub fn new(moqt: MoqtManager) -> Self {
        Self {
            counters: RtmpCounters::default(),
            recorder: None,
            moqt: Some(moqt),
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
            println!("[rtmp {label}] publish app={app_name} stream={stream_key} mode={mode:?}");
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
                let namespace = vec![app_name.clone()];
                if let Err(err) = moqt.setup_publisher(&namespace, &stream_key).await {
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
            if state.counters.audio == 1 || state.counters.audio % 200 == 0 {
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
            if let Some(moqt) = state.moqt.as_ref() {
                let namespace = vec![app_name.clone()];
                if let Err(err) = moqt
                    .send_object(&namespace, &stream_key, b"test^audio")
                    .await
                {
                    eprintln!("[rtmp {label}] moqt send audio failed: {err:?}");
                }
            }
        }
        ServerSessionEvent::VideoDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            state.counters.video += 1;
            if state.counters.video == 1 || state.counters.video % 200 == 0 {
                println!(
                    "[rtmp {label}] video packets={} app={app_name} stream={stream_key}",
                    state.counters.video
                );
            }
            if let Some(recorder) = state.recorder.as_mut() {
                if let Err(err) = recorder.write_video(timestamp.value, data.as_ref()).await {
                    eprintln!("[rtmp {label}] write_video failed: {err:?}");
                }
            }
            if let Some(moqt) = state.moqt.as_ref() {
                let namespace = vec![app_name.clone()];
                if let Err(err) = moqt
                    .send_object(&namespace, &stream_key, b"test-video")
                    .await
                {
                    eprintln!("[rtmp {label}] moqt send video failed: {err:?}");
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

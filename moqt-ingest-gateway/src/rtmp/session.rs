use std::collections::{HashMap, VecDeque};

use anyhow::Result;
use rml_rtmp::sessions::{ServerSession, ServerSessionEvent, ServerSessionResult};

use crate::{
    audio::{AacState, compute_aac_duration_us, pack_audio_chunk_payload},
    ingest::flv::FlvRecorder,
    moqt::MoqtManager,
    video::{AvcState, pack_video_chunk_payload},
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
    pub audio_states: HashMap<(String, String), AacState>,
    pub video_codec_sent: HashMap<(String, String), bool>,
}

impl RtmpState {
    pub fn new(moqt: MoqtManager) -> Self {
        Self {
            counters: RtmpCounters::default(),
            recorder: None,
            moqt: Some(moqt),
            video_states: HashMap::new(),
            audio_states: HashMap::new(),
            video_codec_sent: HashMap::new(),
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
            let namespace_vec: Vec<String> =
                namespace_path.split('/').map(|s| s.to_string()).collect();
            println!(
                "[rtmp {label}] publish app={app_name} stream={cleaned_stream_key} -> ns={namespace_path} mode={mode:?}"
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
                let namespace = namespace_vec;
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
            let (namespace_path, _cleaned_stream_key) =
                split_namespace_and_key(&app_name, &stream_key);
            let key = (namespace_path.clone(), "video".to_string());
            state.video_codec_sent.remove(&key);
            let audio_key = (namespace_path, "audio".to_string());
            state.audio_states.remove(&audio_key);
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
            let (namespace_path, _stream_key) = split_namespace_and_key(&app_name, &stream_key);
            let namespace_vec: Vec<String> =
                namespace_path.split('/').map(|s| s.to_string()).collect();
            let track_name = "audio".to_string();
            state.counters.audio += 1;
            if state.counters.audio == 1 || state.counters.audio % 1000 == 0 {
                println!(
                    "[rtmp {label}] audio packets={} app={app_name} track={track_name}",
                    state.counters.audio
                );
            }
            if let Some(recorder) = state.recorder.as_mut() {
                if let Err(err) = recorder.write_audio(timestamp.value, data.as_ref()).await {
                    eprintln!("[rtmp {label}] write_audio failed: {err:?}");
                }
            }
            if let Some(moqt) = state.moqt.as_ref() {
                let key = (namespace_path.clone(), track_name.clone());
                let frame = match state
                    .audio_states
                    .entry(key)
                    .or_default()
                    .handle_flv_audio(data.as_ref())
                {
                    Ok(f) => f,
                    Err(err) => {
                        eprintln!("[rtmp {label}] aac parse failed: {err:?}");
                        None
                    }
                };
                if let Some(frame) = frame {
                    let timestamp_us = timestamp.value as u64 * 1_000;
                    let duration_us = Some(compute_aac_duration_us(frame.sample_rate));
                    let sent_at_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let payload =
                        pack_audio_chunk_payload(&frame, timestamp_us, duration_us, sent_at_ms);
                    if let Err(err) = moqt
                        .send_object(&namespace_vec, &track_name, false, payload.as_slice())
                        .await
                    {
                        eprintln!("[rtmp {label}] moqt send audio failed: {err:?}");
                    }
                }
            }
        }
        ServerSessionEvent::VideoDataReceived {
            app_name,
            stream_key,
            data,
            timestamp,
        } => {
            let (namespace_path, _stream_key) = split_namespace_and_key(&app_name, &stream_key);
            let namespace_vec: Vec<String> =
                namespace_path.split('/').map(|s| s.to_string()).collect();
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
                let namespace = namespace_vec.as_slice();
                let key = (namespace_path.clone(), track_name.clone());
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
                    let sent_at_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let codec_key = (namespace_path.clone(), track_name.clone());
                    let already_sent = state.video_codec_sent.get(&codec_key).copied().unwrap_or(false);
                    let include_codec = frame.is_key && !already_sent;
                    let payload = pack_video_chunk_payload(
                        frame.is_key,
                        timestamp_us,
                        sent_at_ms,
                        frame.data.as_slice(),
                        if include_codec { frame.codec.as_deref() } else { None },
                        None,
                    );
                    if frame.is_key && include_codec {
                        if let Some(codec) = frame.codec.as_deref() {
                            println!(
                                "[rtmp {label}] detected video codec from SPS: {codec} ns={:?} track={}",
                                namespace,
                                track_name
                            );
                            state.video_codec_sent.insert(codec_key.clone(), true);
                        } else {
                            println!(
                                "[rtmp {label}] video codec from SPS unavailable ns={:?} track={}",
                                namespace, track_name
                            );
                        }
                    }
                    if let Err(err) = moqt
                        .send_object(namespace, &track_name, frame.is_key, payload.as_slice())
                        .await
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

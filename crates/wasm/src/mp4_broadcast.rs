use std::time::Duration;

use broadcast::{Broadcast, BroadcastWriter, MoqtTarget, RelayConnection, SourceTracks};
use bytes::Bytes;
use mediapack::{AudioSample, MediaEvent, Timestamp, VideoSample};
use moqt::executor;
use wasm_bindgen::prelude::*;

use crate::{js_error, mp4::Mp4Index};

/// Publishes the samples of an MP4 file with `crates/broadcast`: the same
/// catalog, LOC and CMAF tracks, media timeline and FETCH cache as
/// live-ingest. The page paces the samples; the broadcast adds no pacing of
/// its own.
#[wasm_bindgen]
pub struct Mp4Broadcast {
    writer: BroadcastWriter,
}

/// One group stream sent to the relay; times are Unix epoch milliseconds.
#[wasm_bindgen]
pub struct PublishedStream {
    #[wasm_bindgen(getter_with_clone)]
    pub track: String,
    #[wasm_bindgen(js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(js_name = openedAtMs)]
    pub opened_at_ms: f64,
    #[wasm_bindgen(js_name = finishedAtMs)]
    pub finished_at_ms: Option<f64>,
    pub bytes: f64,
    #[wasm_bindgen(getter_with_clone, js_name = objectSentAtMs)]
    pub object_sent_at_ms: Vec<f64>,
}

#[wasm_bindgen]
impl Mp4Broadcast {
    /// Queues the codec configurations of `index`; a background task connects
    /// to the relay, and `announce` waits for that connection.
    #[wasm_bindgen(constructor)]
    pub fn new(url: String, namespace: Vec<String>, index: &Mp4Index) -> Result<Self, JsValue> {
        let sample_index = index.sample_index();
        let video = sample_index
            .video
            .clone()
            .ok_or_else(|| js_error("the file has no H.264 video track"))?;
        let height = video
            .sequence_parameter_set()
            .map_err(|err| js_error(format!("{err:#}")))?
            .height;
        let connection = RelayConnection::new(Some(MoqtTarget {
            url,
            auth_token: None,
            verify_certificate: true,
        }));
        let tracks = SourceTracks {
            video_label: format!("{height}p"),
            ..SourceTracks::default()
        };
        let mut writer = BroadcastWriter::run(Broadcast::new(connection, namespace), tracks);
        if let Some(audio) = sample_index.audio.clone() {
            push(&mut writer, MediaEvent::AudioConfig(audio))?;
        }
        push(&mut writer, MediaEvent::VideoConfig(video))?;
        Ok(Self { writer })
    }

    /// Resolves once the relay session exists and the namespace is announced.
    pub async fn announce(&self) -> Result<(), JsValue> {
        self.writer
            .broadcast()
            .announce()
            .await
            .map_err(|err| js_error(format!("{err:#}")))
    }

    /// `data` is Annex-B; `pts_micros` and `dts_micros` count from the start
    /// of the broadcast and must not run backwards across a loop.
    #[wasm_bindgen(js_name = pushVideo)]
    pub fn push_video(
        &mut self,
        data: Vec<u8>,
        is_keyframe: bool,
        pts_micros: f64,
        dts_micros: f64,
    ) -> Result<(), JsValue> {
        push(
            &mut self.writer,
            MediaEvent::Video(VideoSample {
                data: Bytes::from(data),
                is_keyframe,
                pts: Timestamp::from_micros(pts_micros as u64),
                dts: Timestamp::from_micros(dts_micros as u64),
            }),
        )
    }

    /// `pts_micros` counts from the start of the broadcast, as for video.
    #[wasm_bindgen(js_name = pushAudio)]
    pub fn push_audio(&mut self, data: Vec<u8>, pts_micros: f64) -> Result<(), JsValue> {
        push(
            &mut self.writer,
            MediaEvent::Audio(AudioSample {
                data: Bytes::from(data),
                pts: Timestamp::from_micros(pts_micros as u64),
            }),
        )
    }

    /// The open group streams and those that finished within the last 5
    /// seconds.
    pub fn streams(&self) -> Vec<PublishedStream> {
        let epoch_origin = executor::system_time().saturating_sub(executor::now());
        let epoch_ms = |at: Duration| (epoch_origin + at).as_secs_f64() * 1_000.0;
        self.writer
            .broadcast()
            .connection()
            .streams()
            .into_iter()
            .map(|stream| PublishedStream {
                track: stream.track_name,
                group_id: stream.group_id,
                opened_at_ms: epoch_ms(stream.opened_at),
                finished_at_ms: stream.finished_at.map(epoch_ms),
                bytes: stream.bytes as f64,
                object_sent_at_ms: stream.object_sent_at.into_iter().map(epoch_ms).collect(),
            })
            .collect()
    }

    /// Resolves once the CONNECTION_CLOSE has been sent.
    pub async fn close(&self) {
        self.writer.broadcast().connection().close().await;
    }
}

fn push(writer: &mut BroadcastWriter, event: MediaEvent) -> Result<(), JsValue> {
    writer
        .push(event)
        .map_err(|err| js_error(format!("{err:#}")))
}

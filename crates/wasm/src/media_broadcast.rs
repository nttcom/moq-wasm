use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp, aac::AudioSpecificConfig,
    h264::ParameterSetTracker,
};
use wasm_bindgen::prelude::*;

use crate::js_error;

/// An MSF broadcast of media the page encodes: it publishes the catalog, the
/// LOC tracks and the media timeline, keeps objects for FETCH and answers
/// SUBSCRIBE. Created with `Session.broadcast`.
#[wasm_bindgen]
pub struct MediaBroadcast {
    writer: BroadcastWriter,
    parameter_sets: ParameterSetTracker,
}

impl MediaBroadcast {
    pub(crate) fn new(
        connection: RelayConnection,
        namespace: Vec<String>,
        video_label: Option<String>,
    ) -> Self {
        let mut tracks = SourceTracks {
            cmaf: false,
            ..SourceTracks::default()
        };
        if let Some(video_label) = video_label {
            tracks.video_label = video_label;
        }
        Self {
            writer: BroadcastWriter::run(Broadcast::new(connection, namespace), tracks),
            parameter_sets: ParameterSetTracker::new(),
        }
    }

    fn push(&mut self, event: MediaEvent) -> Result<(), JsValue> {
        self.writer
            .push(event)
            .map_err(|err| js_error(format!("{err:#}")))
    }
}

#[wasm_bindgen]
impl MediaBroadcast {
    /// Resolves once the namespace is announced (PUBLISH_NAMESPACE).
    pub async fn announce(&self) -> Result<(), JsValue> {
        self.writer
            .broadcast()
            .announce()
            .await
            .map_err(|err| js_error(format!("{err:#}")))
    }

    /// An H.264 access unit in Annex-B, as a WebCodecs encoder with
    /// `avc: { format: 'annexb' }` emits it. The SPS and PPS a keyframe carries
    /// make the track's configuration, published again when they change.
    /// `timestamp_micros` must not run backwards.
    #[wasm_bindgen(js_name = pushVideo)]
    pub fn push_video(&mut self, data: Vec<u8>, timestamp_micros: f64) -> Result<(), JsValue> {
        let Some(access_unit) = self
            .parameter_sets
            .track(&data)
            .map_err(|err| js_error(format!("{err:#}")))?
        else {
            return Ok(());
        };
        let timestamp = Timestamp::from_micros(timestamp_micros as u64);
        for event in access_unit.into_events(timestamp, timestamp) {
            self.push(event)?;
        }
        Ok(())
    }

    /// `codec` is the WebCodecs codec string: `opus`, or an AAC `mp4a.40.*`
    /// whose `description` is the AudioSpecificConfig.
    #[wasm_bindgen(js_name = configureAudio)]
    pub fn configure_audio(
        &mut self,
        codec: String,
        sample_rate: u32,
        channels: u8,
        description: Option<Vec<u8>>,
    ) -> Result<(), JsValue> {
        let config = match codec.as_str() {
            "opus" => AudioConfig::Opus {
                sample_rate,
                channels,
            },
            aac if aac.starts_with("mp4a.40.") => {
                let description =
                    description.ok_or_else(|| js_error("AAC needs its AudioSpecificConfig"))?;
                AudioConfig::Aac(
                    AudioSpecificConfig::parse(&description)
                        .map_err(|err| js_error(format!("{err:#}")))?,
                )
            }
            other => return Err(js_error(format!("unsupported audio codec: {other}"))),
        };
        self.push(MediaEvent::AudioConfig(config))
    }

    /// One encoded audio frame; call `configureAudio` first. `level` is the
    /// frame's RFC 6464 level in -dBov (0 to 127), written with
    /// `voice_activity` as its LOC Audio Level property.
    #[wasm_bindgen(js_name = pushAudio)]
    pub fn push_audio(
        &mut self,
        data: Vec<u8>,
        timestamp_micros: f64,
        level: Option<u8>,
        voice_activity: Option<bool>,
    ) -> Result<(), JsValue> {
        self.push(MediaEvent::Audio(AudioSample {
            data: Bytes::from(data),
            pts: Timestamp::from_micros(timestamp_micros as u64),
            audio_level: level.map(|level| AudioLevel {
                level,
                voice_activity: voice_activity.unwrap_or(false),
            }),
        }))
    }
}

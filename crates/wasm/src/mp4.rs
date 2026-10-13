use mediapack::mp4::{Mp4Index as SampleIndex, SampleKind};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Mp4VideoTrack {
    #[wasm_bindgen(getter_with_clone)]
    pub codec: String,
    pub width: u32,
    pub height: u32,
}

#[wasm_bindgen]
pub struct Mp4AudioTrack {
    #[wasm_bindgen(getter_with_clone)]
    pub codec: String,
    #[wasm_bindgen(js_name = sampleRate)]
    pub sample_rate: u32,
    pub channels: u8,
}

/// The sample table as parallel columns, one entry per sample in decode
/// order: `kind` is 0 for video and 1 for audio, `sync` is 1 for a keyframe.
#[wasm_bindgen(getter_with_clone)]
pub struct Mp4SampleTable {
    pub kind: Vec<u8>,
    pub offset: Vec<f64>,
    pub size: Vec<u32>,
    #[wasm_bindgen(js_name = dtsMicros)]
    pub dts_micros: Vec<f64>,
    #[wasm_bindgen(js_name = ptsMicros)]
    pub pts_micros: Vec<f64>,
    pub sync: Vec<u8>,
}

/// Indexes a progressive MP4 from its `moov` atom, so a page can read the
/// samples of a large file from a `Blob` range by range instead of loading
/// the whole file into wasm memory.
#[wasm_bindgen]
pub struct Mp4Index {
    inner: SampleIndex,
}

impl Mp4Index {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn sample_index(&self) -> &SampleIndex {
        &self.inner
    }
}

#[wasm_bindgen]
impl Mp4Index {
    #[wasm_bindgen(constructor)]
    pub fn new(moov: &[u8]) -> Result<Mp4Index, JsValue> {
        let inner = SampleIndex::parse(moov).map_err(|err| JsValue::from_str(&err.to_string()))?;
        Ok(Self { inner })
    }

    pub fn video(&self) -> Result<Option<Mp4VideoTrack>, JsValue> {
        let Some(config) = &self.inner.video else {
            return Ok(None);
        };
        let sps = config
            .sequence_parameter_set()
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        Ok(Some(Mp4VideoTrack {
            codec: config.codec_string(),
            width: sps.width,
            height: sps.height,
        }))
    }

    pub fn audio(&self) -> Option<Mp4AudioTrack> {
        let config = self.inner.audio.as_ref()?;
        Some(Mp4AudioTrack {
            codec: config.codec_string(),
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        })
    }

    pub fn samples(&self) -> Mp4SampleTable {
        let samples = self.inner.samples();
        Mp4SampleTable {
            kind: samples
                .iter()
                .map(|sample| (sample.kind == SampleKind::Audio) as u8)
                .collect(),
            offset: samples.iter().map(|sample| sample.offset as f64).collect(),
            size: samples.iter().map(|sample| sample.size).collect(),
            dts_micros: samples
                .iter()
                .map(|sample| sample.dts.micros() as f64)
                .collect(),
            pts_micros: samples
                .iter()
                .map(|sample| sample.pts.micros() as f64)
                .collect(),
            sync: samples.iter().map(|sample| sample.is_sync as u8).collect(),
        }
    }

    #[wasm_bindgen(js_name = reorderDelayMicros)]
    pub fn reorder_delay_micros(&self) -> f64 {
        self.inner.reorder_delay().micros() as f64
    }

    #[wasm_bindgen(js_name = durationMicros)]
    pub fn duration_micros(&self) -> f64 {
        self.inner.duration().micros() as f64
    }

    #[wasm_bindgen(js_name = annexBVideoSample)]
    pub fn annexb_video_sample(&self, avcc: &[u8], sync: bool) -> Result<Vec<u8>, JsValue> {
        self.inner
            .annexb_video_sample(avcc, sync)
            .map(|annexb| annexb.to_vec())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }
}

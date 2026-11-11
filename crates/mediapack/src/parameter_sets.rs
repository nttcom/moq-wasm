use anyhow::Result;
use bytes::Bytes;

use crate::{
    h264::{
        annexb::{nal_units, with_start_codes},
        avcc::length_prefixed,
    },
    sample::{MediaEvent, Timestamp, VideoConfig, VideoSample},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalKind {
    /// A slice of a picture a decoder can start at.
    KeyframeSlice,
    /// A parameter set, by the index of its kind in
    /// `NalDecoderConfig::parameter_sets`.
    ParameterSet(usize),
    AccessUnitDelimiter,
    Other,
}

/// The decoder configuration record of a codec whose access units are NAL
/// units: H.264 or H.265.
pub trait NalDecoderConfig: Clone + PartialEq + Into<VideoConfig> {
    const PARAMETER_SET_KINDS: usize;
    /// The index, in `parameter_sets`, of the kind a keyframe that can be
    /// decoded on its own carries.
    const SEQUENCE_PARAMETER_SET: usize;

    fn nal_kind(nal: &[u8]) -> NalKind;

    /// `sets` holds one list per kind, in the order `parameter_sets` gives.
    fn from_parameter_sets(sets: Vec<Vec<Bytes>>) -> Result<Self>;

    /// One list per kind, in the order a decoder needs them.
    fn parameter_sets(&self) -> Vec<&[Bytes]>;

    fn parse(data: &[u8]) -> Result<Self>;

    fn nal_length_size(&self) -> u8;

    fn parameter_sets_annexb(&self) -> Bytes {
        with_start_codes(
            self.parameter_sets()
                .into_iter()
                .flatten()
                .map(|set| set.as_ref()),
        )
    }

    /// Keyframes must be decodable on their own, so a keyframe that carries no
    /// sequence parameter set gets the record's parameter sets. They follow an
    /// access unit delimiter, which H.264 §7.4.1.2.3 and H.265 §7.4.2.4.4
    /// require to stay first.
    fn with_parameter_sets(&self, annexb: Bytes) -> Bytes {
        let units: Vec<&[u8]> = nal_units(&annexb).collect();
        if units
            .iter()
            .any(|nal| Self::nal_kind(nal) == NalKind::ParameterSet(Self::SEQUENCE_PARAMETER_SET))
        {
            return annexb;
        }
        let parameter_sets = self.parameter_sets_annexb();
        let Some((delimiter, rest)) = units
            .split_first()
            .filter(|(first, _)| Self::nal_kind(first) == NalKind::AccessUnitDelimiter)
        else {
            return Bytes::from([parameter_sets.as_ref(), &annexb].concat());
        };
        with_start_codes(
            [*delimiter]
                .into_iter()
                .chain(nal_units(&parameter_sets))
                .chain(rest.iter().copied()),
        )
    }

    /// The access unit as an MP4 sample: length-prefixed, without the
    /// parameter sets its sample entry holds.
    fn sample_without_parameter_sets(annexb: &[u8], nal_length_size: usize) -> Bytes {
        let nals = nal_units(annexb)
            .filter(|nal| !matches!(Self::nal_kind(nal), NalKind::ParameterSet(_)));
        length_prefixed(nals, nal_length_size)
    }
}

pub struct ParameterSetTracker<C> {
    sets: Vec<Vec<Bytes>>,
    config: Option<C>,
}

pub struct TrackedAccessUnit<C> {
    pub data: Bytes,
    pub is_keyframe: bool,
    pub config_changed: Option<C>,
}

impl<C: NalDecoderConfig> TrackedAccessUnit<C> {
    /// The `VideoConfig` first when the parameter sets changed, then the
    /// sample.
    pub fn into_events(self, pts: Timestamp, dts: Timestamp) -> Vec<MediaEvent> {
        let config = self
            .config_changed
            .map(|config| MediaEvent::VideoConfig(config.into()));
        let sample = MediaEvent::Video(VideoSample {
            data: self.data,
            is_keyframe: self.is_keyframe,
            pts,
            dts,
        });
        config.into_iter().chain([sample]).collect()
    }
}

impl<C: NalDecoderConfig> Default for ParameterSetTracker<C> {
    fn default() -> Self {
        Self {
            sets: vec![Vec::new(); C::PARAMETER_SET_KINDS],
            config: None,
        }
    }
}

impl<C: NalDecoderConfig> ParameterSetTracker<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts with parameter sets announced out of band, so a keyframe that
    /// arrives without them is still completed and the configuration is not
    /// reported again unless the in-band sets differ.
    pub fn from_record(record: &C) -> Self {
        Self {
            sets: record
                .parameter_sets()
                .into_iter()
                .map(<[Bytes]>::to_vec)
                .collect(),
            config: Some(record.clone()),
        }
    }

    pub fn track(&mut self, annexb: &[u8]) -> Result<Option<TrackedAccessUnit<C>>> {
        let nals: Vec<&[u8]> = nal_units(annexb).collect();
        if nals.is_empty() {
            return Ok(None);
        }
        let mut is_keyframe = false;
        let mut inline = vec![Vec::new(); C::PARAMETER_SET_KINDS];
        for nal in &nals {
            match C::nal_kind(nal) {
                NalKind::KeyframeSlice => is_keyframe = true,
                NalKind::ParameterSet(kind) => inline[kind].push(Bytes::copy_from_slice(nal)),
                NalKind::AccessUnitDelimiter | NalKind::Other => {}
            }
        }
        for (known, inline) in self.sets.iter_mut().zip(inline) {
            if !inline.is_empty() {
                *known = inline;
            }
        }
        let mut config_changed = None;
        if self.sets.iter().all(|sets| !sets.is_empty()) {
            let config = C::from_parameter_sets(self.sets.clone())?;
            if self.config.as_ref() != Some(&config) {
                self.config = Some(config.clone());
                config_changed = Some(config);
            }
        }
        let data = with_start_codes(nals);
        let data = match &self.config {
            Some(config) if is_keyframe => config.with_parameter_sets(data),
            _ => data,
        };
        Ok(Some(TrackedAccessUnit {
            data,
            is_keyframe,
            config_changed,
        }))
    }
}

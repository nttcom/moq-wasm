use anyhow::Result;
use bytes::Bytes;

use crate::h264::{
    AvcDecoderConfigurationRecord, NalUnitType,
    annexb::{nal_units, with_start_codes},
    nal::nal_unit_type,
};

#[derive(Default)]
pub struct ParameterSetTracker {
    sps: Vec<Bytes>,
    pps: Vec<Bytes>,
    config: Option<AvcDecoderConfigurationRecord>,
}

pub struct TrackedAccessUnit {
    pub data: Bytes,
    pub is_keyframe: bool,
    pub config_changed: Option<AvcDecoderConfigurationRecord>,
}

impl ParameterSetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(&mut self, annexb: &[u8]) -> Result<Option<TrackedAccessUnit>> {
        let nals: Vec<&[u8]> = nal_units(annexb).collect();
        if nals.is_empty() {
            return Ok(None);
        }
        let mut is_keyframe = false;
        let mut inline_sps = Vec::new();
        let mut inline_pps = Vec::new();
        for nal in &nals {
            match nal_unit_type(nal) {
                Some(NalUnitType::IdrSlice) => is_keyframe = true,
                Some(NalUnitType::Sps) => inline_sps.push(Bytes::copy_from_slice(nal)),
                Some(NalUnitType::Pps) => inline_pps.push(Bytes::copy_from_slice(nal)),
                _ => {}
            }
        }
        let has_inline_sps = !inline_sps.is_empty();
        if has_inline_sps {
            self.sps = inline_sps;
        }
        if !inline_pps.is_empty() {
            self.pps = inline_pps;
        }
        let mut config_changed = None;
        if !self.sps.is_empty() && !self.pps.is_empty() {
            let config = AvcDecoderConfigurationRecord::from_parameter_sets(
                self.sps.clone(),
                self.pps.clone(),
            )?;
            if self.config.as_ref() != Some(&config) {
                self.config = Some(config.clone());
                config_changed = Some(config);
            }
        }
        let data = with_start_codes(nals);
        let data = if is_keyframe {
            self.config.as_ref().map_or_else(
                || data.clone(),
                |config| config.with_parameter_sets(data.clone()),
            )
        } else {
            data
        };
        Ok(Some(TrackedAccessUnit {
            data,
            is_keyframe,
            config_changed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{IDR_SLICE, delta_frame_annexb, fixture_record, keyframe_annexb};

    #[test]
    fn reports_config_once_and_keeps_inline_parameter_sets() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();

        // Act
        let first = tracker.track(&keyframe_annexb()).unwrap().unwrap();
        let second = tracker.track(&keyframe_annexb()).unwrap().unwrap();

        // Assert
        assert_eq!(first.config_changed, Some(fixture_record()));
        assert!(first.is_keyframe);
        assert_eq!(first.data, keyframe_annexb());
        assert!(second.config_changed.is_none());
    }

    #[test]
    fn prepends_known_parameter_sets_to_bare_keyframes() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();
        tracker.track(&keyframe_annexb()).unwrap();

        // Act
        let unit = tracker
            .track(&with_start_codes([&IDR_SLICE[..]]))
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(unit.data, keyframe_annexb());
        assert!(unit.is_keyframe);
    }

    #[test]
    fn leaves_delta_frames_untouched() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();

        // Act
        let unit = tracker.track(&delta_frame_annexb()).unwrap().unwrap();

        // Assert
        assert!(!unit.is_keyframe);
        assert!(unit.config_changed.is_none());
        assert_eq!(unit.data, delta_frame_annexb());
    }

    #[test]
    fn ignores_payload_without_nal_units() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();

        // Act / Assert
        assert!(tracker.track(&[0, 0, 0]).unwrap().is_none());
    }
}

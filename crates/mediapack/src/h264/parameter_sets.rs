use crate::{h264::AvcDecoderConfigurationRecord, parameter_sets};

pub type ParameterSetTracker = parameter_sets::ParameterSetTracker<AvcDecoderConfigurationRecord>;
pub type TrackedAccessUnit = parameter_sets::TrackedAccessUnit<AvcDecoderConfigurationRecord>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h264::annexb::with_start_codes,
        sample::{MediaEvent, Timestamp},
        test_support::{
            IDR_SLICE, delta_frame_annexb, fixture_record, fixture_video_config, keyframe_annexb,
            keyframe_sample,
        },
    };

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
    fn completes_bare_keyframes_from_an_out_of_band_record() {
        // Arrange
        let mut tracker = ParameterSetTracker::from_record(&fixture_record());

        // Act
        let unit = tracker
            .track(&with_start_codes([&IDR_SLICE[..]]))
            .unwrap()
            .unwrap();

        // Assert: the record is already known, so it is not reported again
        assert_eq!(unit.data, keyframe_annexb());
        assert!(unit.config_changed.is_none());
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
    fn a_changed_config_precedes_the_sample_it_came_with() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();
        let pts = Timestamp::from_millis(40);

        // Act
        let events = tracker
            .track(&keyframe_annexb())
            .unwrap()
            .unwrap()
            .into_events(pts, pts);

        // Assert
        assert_eq!(events[0], MediaEvent::VideoConfig(fixture_video_config()));
        assert_eq!(events[1], MediaEvent::Video(keyframe_sample(pts)));
    }

    #[test]
    fn ignores_payload_without_nal_units() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();

        // Act / Assert
        assert!(tracker.track(&[0, 0, 0]).unwrap().is_none());
    }
}

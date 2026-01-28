use super::{NumericField, FIELD_PAN, FIELD_SPEED, FIELD_TILT, FIELD_ZOOM, STEP_TENTH};
use crate::ptz_config::PtzRange;

pub(super) fn default_fields(range: &PtzRange) -> Vec<NumericField> {
    let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
    let zoom_range = range.absolute_zoom_range();
    let speed_range = range.speed_range();
    let pan = pan_range.max;
    let tilt = tilt_range.clamp(0.0);
    let zoom = zoom_range.clamp(0.0);
    let speed = range.speed_default.clamp(speed_range.min, speed_range.max);
    vec![
        NumericField::new(FIELD_PAN, pan, pan_range.min, pan_range.max, STEP_TENTH),
        NumericField::new(FIELD_TILT, tilt, tilt_range.min, tilt_range.max, STEP_TENTH),
        NumericField::new(FIELD_ZOOM, zoom, zoom_range.min, zoom_range.max, STEP_TENTH),
        NumericField::new(
            FIELD_SPEED,
            speed,
            speed_range.min,
            speed_range.max,
            STEP_TENTH,
        ),
    ]
}

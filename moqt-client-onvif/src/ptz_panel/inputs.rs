use super::UiCommand;
use crate::ptz_config::PtzRange;

pub(super) struct CommandInputs {
    pub(super) absolute: MoveInputs,
    pub(super) relative: MoveInputs,
    pub(super) continuous: MoveInputs,
    pub(super) stop: MoveInputs,
    pub(super) center: MoveInputs,
}

impl CommandInputs {
    pub(super) fn new(range: &PtzRange) -> Self {
        let defaults = MoveInputs::new(range);
        Self {
            absolute: defaults.clone(),
            relative: defaults.clone(),
            continuous: defaults.clone(),
            stop: defaults.clone(),
            center: defaults,
        }
    }

    pub(super) fn for_command_mut(&mut self, command: UiCommand) -> &mut MoveInputs {
        match command {
            UiCommand::Move => &mut self.absolute,
            UiCommand::Relative => &mut self.relative,
            UiCommand::Continuous => &mut self.continuous,
            UiCommand::Stop => &mut self.stop,
            UiCommand::Center => &mut self.center,
        }
    }
}

#[derive(Clone)]
pub(super) struct MoveInputs {
    pub(super) pan: String,
    pub(super) tilt: String,
    pub(super) zoom: String,
    pub(super) speed: String,
}

impl MoveInputs {
    fn new(range: &PtzRange) -> Self {
        let (pan, tilt, zoom, speed) = default_move_values(range);
        Self {
            pan: format_value(pan),
            tilt: format_value(tilt),
            zoom: format_value(zoom),
            speed: format_value(speed),
        }
    }
}

pub(super) fn parse_move_values(
    label: &str,
    inputs: &mut MoveInputs,
) -> Result<(f32, f32, f32, f32), String> {
    let pan = parse_value(label, "pan", &mut inputs.pan)?;
    let tilt = parse_value(label, "tilt", &mut inputs.tilt)?;
    let zoom = parse_value(label, "zoom", &mut inputs.zoom)?;
    let speed = parse_value(label, "speed", &mut inputs.speed)?;
    Ok((pan, tilt, zoom, speed))
}

pub(super) fn parse_speed_value(label: &str, inputs: &mut MoveInputs) -> Result<f32, String> {
    parse_value(label, "speed", &mut inputs.speed)
}

pub(super) fn snap_value(input: &mut String) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(value) = trimmed.parse::<f32>() {
        *input = format_value(round_to_tenth(value));
    }
}

fn parse_value(label: &str, field: &str, input: &mut String) -> Result<f32, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("{label}: {field} is required"));
    }
    let parsed = trimmed
        .parse::<f32>()
        .map_err(|_| format!("{label}: invalid {field} value '{trimmed}'"))?;
    let snapped = round_to_tenth(parsed);
    *input = format_value(snapped);
    Ok(snapped)
}

fn round_to_tenth(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

fn default_move_values(range: &PtzRange) -> (f32, f32, f32, f32) {
    let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
    let zoom_range = range.absolute_zoom_range();
    let speed_range = range.speed_range();
    let pan = pan_range.max;
    let tilt = tilt_range.clamp(0.0);
    let zoom = zoom_range.clamp(0.0);
    let speed = range.speed_default.clamp(speed_range.min, speed_range.max);
    (pan, tilt, zoom, speed)
}

fn format_value(value: f32) -> String {
    format!("{value:.1}")
}

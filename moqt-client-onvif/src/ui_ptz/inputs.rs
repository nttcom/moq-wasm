use super::UiCommand;
use crate::ptz_config::{AxisRange, PtzRange};

pub(super) const STEP_TENTH: f32 = 0.1;

const FIELD_PAN: &str = "Pan";
const FIELD_TILT: &str = "Tilt";
const FIELD_ZOOM: &str = "Zoom";
const FIELD_SPEED: &str = "Speed";

pub(super) struct CommandInputs {
    pub(super) absolute: MoveInputs,
    pub(super) relative: MoveInputs,
    pub(super) continuous: MoveInputs,
    pub(super) stop: MoveInputs,
    pub(super) center: MoveInputs,
}

impl CommandInputs {
    pub(super) fn new(range: &PtzRange) -> Self {
        Self {
            absolute: MoveInputs::new(range, UiCommand::Move),
            relative: MoveInputs::new(range, UiCommand::Relative),
            continuous: MoveInputs::new(range, UiCommand::Continuous),
            stop: MoveInputs::new(range, UiCommand::Stop),
            center: MoveInputs::new(range, UiCommand::Center),
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

    pub(super) fn field_labels(&self) -> Vec<&'static str> {
        self.absolute.labels()
    }
}

#[derive(Clone)]
pub(super) struct MoveInputs {
    fields: Vec<NumericField>,
}

impl MoveInputs {
    fn new(range: &PtzRange, command: UiCommand) -> Self {
        Self {
            fields: fields_for_command(range, command),
        }
    }

    pub(super) fn field_mut(&mut self, label: &str) -> &mut NumericField {
        self.fields
            .iter_mut()
            .find(|field| field.label == label)
            .expect("missing PTZ input field")
    }

    pub(super) fn labels(&self) -> Vec<&'static str> {
        self.fields.iter().map(|field| field.label).collect()
    }
}

#[derive(Clone)]
pub(super) struct NumericField {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) step: f32,
}

impl NumericField {
    fn new(label: &'static str, value: f32, min: f32, max: f32, step: f32) -> Self {
        Self {
            label,
            value: format_value(value),
            min,
            max,
            step,
        }
    }
}

pub(super) fn parse_move_values(
    label: &str,
    inputs: &mut MoveInputs,
) -> Result<(f32, f32, f32, f32), String> {
    let pan = parse_field(label, inputs.field_mut(FIELD_PAN))?;
    let tilt = parse_field(label, inputs.field_mut(FIELD_TILT))?;
    let zoom = parse_field(label, inputs.field_mut(FIELD_ZOOM))?;
    let speed = parse_field(label, inputs.field_mut(FIELD_SPEED))?;
    Ok((pan, tilt, zoom, speed))
}

pub(super) fn parse_speed_value(label: &str, inputs: &mut MoveInputs) -> Result<f32, String> {
    parse_field(label, inputs.field_mut(FIELD_SPEED))
}

pub(super) fn snap_field(field: &mut NumericField) -> Option<String> {
    let trimmed = field.value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let field_name = field.label.to_ascii_lowercase();
    let parsed = match trimmed.parse::<f32>() {
        Ok(value) => value,
        Err(_) => {
            return Some(format!("invalid {field_name} value '{trimmed}'"));
        }
    };
    let snapped = round_to_step(parsed, field.step);
    if snapped < field.min || snapped > field.max {
        return Some(format!(
            "{field_name} {snapped:.1} is out of range ({:.1}..{:.1})",
            field.min, field.max
        ));
    }
    field.value = format_value(snapped);
    None
}

fn fields_for_command(range: &PtzRange, command: UiCommand) -> Vec<NumericField> {
    let (pan_range, tilt_range, zoom_range) = command_ranges(range, command);
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

fn command_ranges(range: &PtzRange, command: UiCommand) -> (AxisRange, AxisRange, AxisRange) {
    match command {
        UiCommand::Move | UiCommand::Center | UiCommand::Stop => {
            let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
            let zoom_range = range.absolute_zoom_range();
            (pan_range, tilt_range, zoom_range)
        }
        UiCommand::Relative => {
            let (pan_range, tilt_range) = range.relative_pan_tilt_range();
            let zoom_range = range.relative_zoom_range();
            (pan_range, tilt_range, zoom_range)
        }
        UiCommand::Continuous => {
            let (pan_range, tilt_range) = range.continuous_pan_tilt_range();
            let zoom_range = range.continuous_zoom_range();
            (pan_range, tilt_range, zoom_range)
        }
    }
}

fn parse_field(label: &str, field: &mut NumericField) -> Result<f32, String> {
    let field_name = field.label.to_ascii_lowercase();
    parse_value(label, &field_name, field)
}

fn parse_value(label: &str, field_name: &str, field: &mut NumericField) -> Result<f32, String> {
    let trimmed = field.value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label}: {field_name} is required"));
    }
    let parsed = trimmed
        .parse::<f32>()
        .map_err(|_| format!("{label}: invalid {field_name} value '{trimmed}'"))?;
    let snapped = round_to_step(parsed, field.step);
    if snapped < field.min || snapped > field.max {
        return Err(format!(
            "{label}: {field_name} {snapped:.1} is out of range ({:.1}..{:.1})",
            field.min, field.max
        ));
    }
    field.value = format_value(snapped);
    Ok(snapped)
}

fn round_to_step(value: f32, step: f32) -> f32 {
    if step == 0.0 {
        return value;
    }
    (value / step).round() * step
}

fn format_value(value: f32) -> String {
    format!("{value:.1}")
}

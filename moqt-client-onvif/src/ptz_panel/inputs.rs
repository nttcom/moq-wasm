mod defaults;
mod parse;

use super::UiCommand;
use crate::ptz_config::PtzRange;

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

    pub(super) fn field_labels(&self) -> Vec<&'static str> {
        self.absolute.labels()
    }
}

#[derive(Clone)]
pub(super) struct MoveInputs {
    fields: Vec<NumericField>,
}

impl MoveInputs {
    fn new(range: &PtzRange) -> Self {
        Self {
            fields: defaults::default_fields(range),
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
            value: parse::format_value(value),
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
    parse::parse_move_values(label, inputs)
}

pub(super) fn parse_speed_value(label: &str, inputs: &mut MoveInputs) -> Result<f32, String> {
    parse::parse_speed_value(label, inputs)
}

pub(super) fn snap_field(field: &mut NumericField) {
    parse::snap_field(field);
}

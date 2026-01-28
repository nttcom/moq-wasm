use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::PtzRange;
use crate::ptz_state::PtzState;
use crate::ptz_worker::Command;
use eframe::egui;
use strum::{EnumIter, IntoEnumIterator};
pub struct Controls {
    range: PtzRange,
    node_info: Option<PtzNodeInfo>,
    inputs: CommandInputs,
    input_error: Option<String>,
}
impl Controls {
    pub fn new() -> Self {
        let range = PtzRange::default();
        let inputs = CommandInputs::new(&range);
        Self {
            range,
            node_info: None,
            inputs,
            input_error: None,
        }
    }

    pub fn set_state(&mut self, state: PtzState) {
        self.set_range(state.range);
        self.node_info = state.node;
    }

    fn set_range(&mut self, range: PtzRange) {
        self.range = range;
        self.inputs = CommandInputs::new(&self.range);
        self.input_error = None;
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        ui.heading("PTZ Control");
        let mut pending = None;
        let column_count = UiCommand::iter().count() + 1;
        egui::Grid::new("ptz_command_grid")
            .num_columns(column_count)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label("");
                for command in UiCommand::iter() {
                    let enabled = self.command_supported(command);
                    let clicked = ui
                        .add_enabled(enabled, egui::Button::new(command.label()))
                        .clicked();
                    if clicked && pending.is_none() {
                        pending = self.command_from_input(command);
                    }
                }
                ui.end_row();
                ui.label("Pan");
                for command in UiCommand::iter() {
                    let input = &mut self.inputs.for_command_mut(command).pan;
                    input_cell(ui, input);
                }
                ui.end_row();
                ui.label("Tilt");
                for command in UiCommand::iter() {
                    let input = &mut self.inputs.for_command_mut(command).tilt;
                    input_cell(ui, input);
                }
                ui.end_row();
                ui.label("Zoom");
                for command in UiCommand::iter() {
                    let input = &mut self.inputs.for_command_mut(command).zoom;
                    input_cell(ui, input);
                }
                ui.end_row();
                ui.label("Speed");
                for command in UiCommand::iter() {
                    let input = &mut self.inputs.for_command_mut(command).speed;
                    input_cell(ui, input);
                }
                ui.end_row();
            });
        if let Some(error) = &self.input_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::RED, error);
        }
        egui::CollapsingHeader::new("PTZ Ranges")
            .default_open(false)
            .show(ui, |ui| {
                for line in self.range.summary_lines() {
                    ui.label(line);
                }
            });
        egui::CollapsingHeader::new("PTZ Node")
            .default_open(false)
            .show(ui, |ui| {
                if let Some(node) = &self.node_info {
                    for line in node.summary_lines() {
                        ui.label(line);
                    }
                } else {
                    ui.label("PTZ node: (not available)");
                }
            });
        pending
    }

    fn command_from_input(&mut self, command: UiCommand) -> Option<Command> {
        let label = command.label();
        let result = match command {
            UiCommand::Move => {
                let (pan, tilt, zoom, speed) = self.parse_move_for(command, label)?;
                log::info!(
                    "PTZ command: AbsoluteMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    pan,
                    tilt,
                    zoom,
                    speed
                );
                Command::Absolute {
                    pan,
                    tilt,
                    zoom,
                    speed,
                }
            }
            UiCommand::Relative => {
                let (pan, tilt, zoom, speed) = self.parse_move_for(command, label)?;
                log::info!(
                    "PTZ command: RelativeMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    pan,
                    tilt,
                    zoom,
                    speed
                );
                Command::Relative {
                    pan,
                    tilt,
                    zoom,
                    speed,
                }
            }
            UiCommand::Continuous => {
                let (pan, tilt, zoom, speed) = self.parse_move_for(command, label)?;
                log::info!(
                    "PTZ command: ContinuousMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    pan,
                    tilt,
                    zoom,
                    speed
                );
                Command::Continuous {
                    pan,
                    tilt,
                    zoom,
                    speed,
                }
            }
            UiCommand::Stop => {
                log::info!("PTZ command: Stop");
                Command::Stop
            }
            UiCommand::Center => {
                let speed = self.parse_speed_for(command, label)?;
                log::info!("PTZ command: Center speed={:.2}", speed);
                Command::Center { speed }
            }
        };
        self.input_error = None;
        Some(result)
    }

    fn parse_move_for(&mut self, command: UiCommand, label: &str) -> Option<(f32, f32, f32, f32)> {
        let parsed = {
            let inputs = self.inputs.for_command_mut(command);
            parse_move_values(label, inputs)
        };
        match parsed {
            Ok(values) => Some(values),
            Err(error) => {
                self.input_error = Some(error);
                None
            }
        }
    }

    fn parse_speed_for(&mut self, command: UiCommand, label: &str) -> Option<f32> {
        let parsed = {
            let inputs = self.inputs.for_command_mut(command);
            parse_speed_value(label, inputs)
        };
        match parsed {
            Ok(speed) => Some(speed),
            Err(error) => {
                self.input_error = Some(error);
                None
            }
        }
    }

    fn command_supported(&self, command: UiCommand) -> bool {
        match command {
            UiCommand::Move | UiCommand::Center => self.node_supports(
                self.range.absolute_pan_tilt_space(),
                self.range.absolute_zoom_space(),
            ),
            UiCommand::Relative => self.node_supports(
                self.range.relative_pan_tilt_space(),
                self.range.relative_zoom_space(),
            ),
            UiCommand::Continuous => self.node_supports(
                self.range.continuous_pan_tilt_space(),
                self.range.continuous_zoom_space(),
            ),
            UiCommand::Stop => true,
        }
    }

    fn node_supports(&self, pan_tilt_space: Option<&str>, zoom_space: Option<&str>) -> bool {
        let Some(node) = &self.node_info else {
            return true;
        };
        let mut supported = true;
        if let Some(uri) = pan_tilt_space {
            supported &= node.supports_uri(uri);
        }
        if let Some(uri) = zoom_space {
            supported &= node.supports_uri(uri);
        }
        supported
    }
}

fn input_cell(ui: &mut egui::Ui, value: &mut String) {
    let response = ui.add(egui::TextEdit::singleline(value).desired_width(90.0));
    if response.lost_focus() {
        snap_value(value);
    }
}

fn parse_move_values(label: &str, inputs: &mut MoveInputs) -> Result<(f32, f32, f32, f32), String> {
    let pan = parse_value(label, "pan", &mut inputs.pan)?;
    let tilt = parse_value(label, "tilt", &mut inputs.tilt)?;
    let zoom = parse_value(label, "zoom", &mut inputs.zoom)?;
    let speed = parse_value(label, "speed", &mut inputs.speed)?;
    Ok((pan, tilt, zoom, speed))
}

fn parse_speed_value(label: &str, inputs: &mut MoveInputs) -> Result<f32, String> {
    parse_value(label, "speed", &mut inputs.speed)
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

fn snap_value(input: &mut String) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(value) = trimmed.parse::<f32>() {
        *input = format_value(round_to_tenth(value));
    }
}

#[derive(Clone, Copy, PartialEq, EnumIter)]
enum UiCommand {
    Move,
    Relative,
    Continuous,
    Stop,
    Center,
}
impl UiCommand {
    fn label(self) -> &'static str {
        match self {
            UiCommand::Move => "AbsoluteMove",
            UiCommand::Relative => "RelativeMove",
            UiCommand::Continuous => "ContinuousMove",
            UiCommand::Stop => "Stop",
            UiCommand::Center => "Center",
        }
    }
}

struct CommandInputs {
    absolute: MoveInputs,
    relative: MoveInputs,
    continuous: MoveInputs,
    stop: MoveInputs,
    center: MoveInputs,
}

impl CommandInputs {
    fn new(range: &PtzRange) -> Self {
        let defaults = MoveInputs::new(range);
        Self {
            absolute: defaults.clone(),
            relative: defaults.clone(),
            continuous: defaults.clone(),
            stop: defaults.clone(),
            center: defaults,
        }
    }

    fn for_command_mut(&mut self, command: UiCommand) -> &mut MoveInputs {
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
struct MoveInputs {
    pan: String,
    tilt: String,
    zoom: String,
    speed: String,
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

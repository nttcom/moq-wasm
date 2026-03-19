mod inputs;

use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::PtzRange;
use crate::ptz_state::PtzState;
use crate::ptz_worker::Command;
use eframe::egui;
use inputs::{snap_field, CommandInputs, NumericField};
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
        let supported = self.supported_commands();
        let grid = command_grid(ui, &mut self.inputs, &supported);
        if let Some(error) = grid.input_error {
            self.input_error = Some(error);
        } else if grid.input_touched {
            self.input_error = None;
        }
        let pending = grid
            .pending
            .and_then(|command| self.command_from_input(command));
        if let Some(error) = &self.input_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::RED, error);
        }
        ranges_panel(ui, &self.range);
        node_panel(ui, &self.node_info);
        pending
    }

    fn supported_commands(&self) -> Vec<(UiCommand, bool)> {
        UiCommand::iter()
            .map(|command| (command, self.command_supported(command)))
            .collect()
    }

    fn command_from_input(&mut self, command: UiCommand) -> Option<Command> {
        let label = command.label();
        let result = match command {
            UiCommand::Move | UiCommand::Relative | UiCommand::Continuous => {
                self.build_move_command(command, label)?
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

    fn build_move_command(&mut self, command: UiCommand, label: &str) -> Option<Command> {
        let (pan, tilt, zoom, speed) = self.parse_move_for(command, label)?;
        log::info!(
            "PTZ command: {label} pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
            pan,
            tilt,
            zoom,
            speed
        );
        let built = match command {
            UiCommand::Move => Command::Absolute {
                pan,
                tilt,
                zoom,
                speed,
            },
            UiCommand::Relative => Command::Relative {
                pan,
                tilt,
                zoom,
                speed,
            },
            UiCommand::Continuous => Command::Continuous {
                pan,
                tilt,
                zoom,
                speed,
            },
            _ => return None,
        };
        Some(built)
    }

    fn parse_move_for(&mut self, command: UiCommand, label: &str) -> Option<(f32, f32, f32, f32)> {
        match inputs::parse_move_values(label, self.inputs.for_command_mut(command)) {
            Ok(values) => Some(values),
            Err(error) => {
                self.input_error = Some(error);
                None
            }
        }
    }

    fn parse_speed_for(&mut self, command: UiCommand, label: &str) -> Option<f32> {
        match inputs::parse_speed_value(label, self.inputs.for_command_mut(command)) {
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

impl Default for Controls {
    fn default() -> Self {
        Self::new()
    }
}

struct CommandGridResult {
    pending: Option<UiCommand>,
    input_error: Option<String>,
    input_touched: bool,
}

fn command_grid(
    ui: &mut egui::Ui,
    inputs: &mut CommandInputs,
    supported: &[(UiCommand, bool)],
) -> CommandGridResult {
    let column_count = supported.len() + 1;
    let field_labels = inputs.field_labels();
    let mut pending = None;
    let mut input_error = None;
    let mut input_touched = false;
    egui::Grid::new("ptz_command_grid")
        .num_columns(column_count)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label("");
            for (command, enabled) in supported {
                let clicked = ui
                    .add_enabled(*enabled, egui::Button::new(command.label()))
                    .clicked();
                if clicked && pending.is_none() {
                    pending = Some(*command);
                }
            }
            ui.end_row();
            for &label in &field_labels {
                add_input_row(
                    ui,
                    label,
                    inputs,
                    supported,
                    &mut input_error,
                    &mut input_touched,
                );
            }
        });
    CommandGridResult {
        pending,
        input_error,
        input_touched,
    }
}

fn ranges_panel(ui: &mut egui::Ui, range: &PtzRange) {
    egui::CollapsingHeader::new("PTZ Ranges")
        .default_open(false)
        .show(ui, |ui| {
            for line in range.summary_lines() {
                ui.label(line);
            }
        });
}

fn node_panel(ui: &mut egui::Ui, node: &Option<PtzNodeInfo>) {
    egui::CollapsingHeader::new("PTZ Node")
        .default_open(false)
        .show(ui, |ui| {
            if let Some(node) = node {
                for line in node.summary_lines() {
                    ui.label(line);
                }
            } else {
                ui.label("PTZ node: (not available)");
            }
        });
}

fn add_input_row(
    ui: &mut egui::Ui,
    label: &str,
    inputs: &mut CommandInputs,
    supported: &[(UiCommand, bool)],
    input_error: &mut Option<String>,
    input_touched: &mut bool,
) {
    ui.label(label);
    for (command, _) in supported {
        let field = inputs.for_command_mut(*command).field_mut(label);
        if let Some(error) = input_cell(ui, field, command.label(), input_touched) {
            if input_error.is_none() {
                *input_error = Some(error);
            }
        }
    }
    ui.end_row();
}

fn input_cell(
    ui: &mut egui::Ui,
    field: &mut NumericField,
    command_label: &str,
    input_touched: &mut bool,
) -> Option<String> {
    let hint = format!("{:.1}..{:.1}", field.min, field.max);
    let response = ui.add(
        egui::TextEdit::singleline(&mut field.value)
            .desired_width(90.0)
            .hint_text(hint),
    );
    if response.lost_focus() {
        *input_touched = true;
        if let Some(error) = snap_field(field) {
            return Some(format!("{command_label}: {error}"));
        }
    }
    None
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

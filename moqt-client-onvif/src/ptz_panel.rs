mod inputs;
mod layout;

use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::PtzRange;
use crate::ptz_state::PtzState;
use crate::ptz_worker::Command;
use eframe::egui;
use inputs::CommandInputs;
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
        let pending = layout::command_grid(ui, &mut self.inputs, &supported)
            .and_then(|command| self.command_from_input(command));
        if let Some(error) = &self.input_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::RED, error);
        }
        layout::ranges_panel(ui, &self.range);
        layout::node_panel(ui, &self.node_info);
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

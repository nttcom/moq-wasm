use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::{AxisRange, PtzRange};
use crate::ptz_state::PtzState;
use crate::ptz_worker::Command;
use eframe::egui;
pub struct Controls {
    command: UiCommand,
    pan_delta: f32,
    tilt_delta: f32,
    zoom_delta: f32,
    speed: f32,
    range: PtzRange,
    node_info: Option<PtzNodeInfo>,
}
impl Controls {
    pub fn new() -> Self {
        let range = PtzRange::default();
        let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
        let zoom_range = range.absolute_zoom_range();
        let speed_range = range.speed_range();
        Self {
            command: UiCommand::Move,
            pan_delta: pan_range.max,
            tilt_delta: tilt_range.clamp(0.0),
            zoom_delta: zoom_range.clamp(0.0),
            speed: range.speed_default.clamp(speed_range.min, speed_range.max),
            range,
            node_info: None,
        }
    }

    pub fn set_state(&mut self, state: PtzState) {
        self.set_range(state.range);
        self.node_info = state.node;
        self.ensure_supported_command();
    }

    fn set_range(&mut self, range: PtzRange) {
        self.range = range;
        let (pan_range, tilt_range) = self.range.absolute_pan_tilt_range();
        let zoom_range = self.range.absolute_zoom_range();
        let speed_range = self.range.speed_range();
        self.pan_delta = self.pan_delta.clamp(pan_range.min, pan_range.max);
        self.tilt_delta = self.tilt_delta.clamp(tilt_range.min, tilt_range.max);
        self.zoom_delta = self.zoom_delta.clamp(zoom_range.min, zoom_range.max);
        self.speed = self.speed.clamp(speed_range.min, speed_range.max);
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        ui.heading("PTZ Control");
        let (pan_range, tilt_range) = self.pan_tilt_range_for_command();
        let zoom_range = self.zoom_range_for_command();
        let speed_range = self.range.speed_range();
        egui::Grid::new("ptz_controls_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label("Command");
                egui::ComboBox::from_id_source("ptz_command")
                    .selected_text(self.command.label())
                    .show_ui(ui, |ui| {
                        ui.add_enabled_ui(self.command_supported(UiCommand::Move), |ui| {
                            ui.selectable_value(&mut self.command, UiCommand::Move, "AbsoluteMove");
                        });
                        ui.add_enabled_ui(self.command_supported(UiCommand::Relative), |ui| {
                            ui.selectable_value(
                                &mut self.command,
                                UiCommand::Relative,
                                "RelativeMove",
                            );
                        });
                        ui.add_enabled_ui(self.command_supported(UiCommand::Continuous), |ui| {
                            ui.selectable_value(
                                &mut self.command,
                                UiCommand::Continuous,
                                "ContinuousMove",
                            );
                        });
                        ui.add_enabled_ui(self.command_supported(UiCommand::Stop), |ui| {
                            ui.selectable_value(&mut self.command, UiCommand::Stop, "Stop");
                        });
                        ui.add_enabled_ui(self.command_supported(UiCommand::Center), |ui| {
                            ui.selectable_value(&mut self.command, UiCommand::Center, "Center");
                        });
                    });
                ui.end_row();
                ui.label("Pan delta");
                ui.add(
                    egui::DragValue::new(&mut self.pan_delta)
                        .speed(0.05)
                        .clamp_range(pan_range.min..=pan_range.max),
                );
                ui.end_row();
                ui.label("Tilt delta");
                ui.add(
                    egui::DragValue::new(&mut self.tilt_delta)
                        .speed(0.05)
                        .clamp_range(tilt_range.min..=tilt_range.max),
                );
                ui.end_row();
                ui.label("Zoom delta");
                ui.add(
                    egui::DragValue::new(&mut self.zoom_delta)
                        .speed(0.05)
                        .clamp_range(zoom_range.min..=zoom_range.max),
                );
                ui.end_row();
                ui.label("Speed");
                ui.add(
                    egui::DragValue::new(&mut self.speed)
                        .speed(0.05)
                        .clamp_range(speed_range.min..=speed_range.max),
                );
                ui.end_row();
            });
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
        if ui.button("Send").clicked() {
            return Some(self.build_command());
        }
        None
    }
    fn build_command(&self) -> Command {
        match self.command {
            UiCommand::Move => {
                log::info!(
                    "PTZ command: AbsoluteMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    self.pan_delta,
                    self.tilt_delta,
                    self.zoom_delta,
                    self.speed
                );
                Command::Absolute {
                    pan: self.pan_delta,
                    tilt: self.tilt_delta,
                    zoom: self.zoom_delta,
                    speed: self.speed,
                }
            }
            UiCommand::Relative => {
                log::info!(
                    "PTZ command: RelativeMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    self.pan_delta,
                    self.tilt_delta,
                    self.zoom_delta,
                    self.speed
                );
                Command::Relative {
                    pan: self.pan_delta,
                    tilt: self.tilt_delta,
                    zoom: self.zoom_delta,
                    speed: self.speed,
                }
            }
            UiCommand::Continuous => {
                log::info!(
                    "PTZ command: ContinuousMove pan={:.2} tilt={:.2} zoom={:.2} speed={:.2}",
                    self.pan_delta,
                    self.tilt_delta,
                    self.zoom_delta,
                    self.speed
                );
                Command::Continuous {
                    pan: self.pan_delta,
                    tilt: self.tilt_delta,
                    zoom: self.zoom_delta,
                    speed: self.speed,
                }
            }
            UiCommand::Stop => {
                log::info!("PTZ command: Stop");
                Command::Stop
            }
            UiCommand::Center => {
                log::info!("PTZ command: Center speed={:.2}", self.speed);
                Command::Center { speed: self.speed }
            }
        }
    }

    fn pan_tilt_range_for_command(&self) -> (AxisRange, AxisRange) {
        match self.command {
            UiCommand::Move | UiCommand::Center | UiCommand::Stop => {
                self.range.absolute_pan_tilt_range()
            }
            UiCommand::Relative => self.range.relative_pan_tilt_range(),
            UiCommand::Continuous => self.range.continuous_pan_tilt_range(),
        }
    }

    fn zoom_range_for_command(&self) -> AxisRange {
        match self.command {
            UiCommand::Move | UiCommand::Center | UiCommand::Stop => {
                self.range.absolute_zoom_range()
            }
            UiCommand::Relative => self.range.relative_zoom_range(),
            UiCommand::Continuous => self.range.continuous_zoom_range(),
        }
    }

    fn ensure_supported_command(&mut self) {
        if self.command_supported(self.command) {
            return;
        }
        for candidate in [
            UiCommand::Move,
            UiCommand::Relative,
            UiCommand::Continuous,
            UiCommand::Stop,
            UiCommand::Center,
        ] {
            if self.command_supported(candidate) {
                self.command = candidate;
                return;
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
#[derive(Clone, Copy, PartialEq)]
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

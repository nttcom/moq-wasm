use super::inputs::{snap_value, CommandInputs, MoveInputs};
use super::UiCommand;
use crate::onvif_nodes::PtzNodeInfo;
use crate::ptz_config::PtzRange;
use eframe::egui;

pub(super) fn command_grid(
    ui: &mut egui::Ui,
    inputs: &mut CommandInputs,
    supported: &[(UiCommand, bool)],
) -> Option<UiCommand> {
    let column_count = supported.len() + 1;
    let mut pending = None;
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
            add_input_row(ui, "Pan", inputs, supported, InputField::Pan);
            add_input_row(ui, "Tilt", inputs, supported, InputField::Tilt);
            add_input_row(ui, "Zoom", inputs, supported, InputField::Zoom);
            add_input_row(ui, "Speed", inputs, supported, InputField::Speed);
        });
    pending
}

pub(super) fn ranges_panel(ui: &mut egui::Ui, range: &PtzRange) {
    egui::CollapsingHeader::new("PTZ Ranges")
        .default_open(false)
        .show(ui, |ui| {
            for line in range.summary_lines() {
                ui.label(line);
            }
        });
}

pub(super) fn node_panel(ui: &mut egui::Ui, node: &Option<PtzNodeInfo>) {
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

#[derive(Clone, Copy)]
enum InputField {
    Pan,
    Tilt,
    Zoom,
    Speed,
}

fn add_input_row(
    ui: &mut egui::Ui,
    label: &str,
    inputs: &mut CommandInputs,
    supported: &[(UiCommand, bool)],
    field: InputField,
) {
    ui.label(label);
    for (command, _) in supported {
        let value = select_field(inputs.for_command_mut(*command), field);
        input_cell(ui, value);
    }
    ui.end_row();
}

fn select_field(inputs: &mut MoveInputs, field: InputField) -> &mut String {
    match field {
        InputField::Pan => &mut inputs.pan,
        InputField::Tilt => &mut inputs.tilt,
        InputField::Zoom => &mut inputs.zoom,
        InputField::Speed => &mut inputs.speed,
    }
}

fn input_cell(ui: &mut egui::Ui, value: &mut String) {
    let response = ui.add(egui::TextEdit::singleline(value).desired_width(90.0));
    if response.lost_focus() {
        snap_value(value);
    }
}

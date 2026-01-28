use super::inputs::{snap_field, CommandInputs, NumericField};
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
    let field_labels = inputs.field_labels();
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
            for &label in &field_labels {
                add_input_row(ui, label, inputs, supported);
            }
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

fn add_input_row(
    ui: &mut egui::Ui,
    label: &str,
    inputs: &mut CommandInputs,
    supported: &[(UiCommand, bool)],
) {
    ui.label(label);
    for (command, _) in supported {
        let field = inputs.for_command_mut(*command).field_mut(label);
        input_cell(ui, field);
    }
    ui.end_row();
}

fn input_cell(ui: &mut egui::Ui, field: &mut NumericField) {
    let hint = format!("{:.1}..{:.1}", field.min, field.max);
    let response = ui.add(
        egui::TextEdit::singleline(&mut field.value)
            .desired_width(90.0)
            .hint_text(hint),
    );
    if response.lost_focus() {
        snap_field(field);
    }
}

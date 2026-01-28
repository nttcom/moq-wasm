use super::{MoveInputs, NumericField, FIELD_PAN, FIELD_SPEED, FIELD_TILT, FIELD_ZOOM};

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

pub(super) fn snap_field(field: &mut NumericField) {
    let trimmed = field.value.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(value) = trimmed.parse::<f32>() {
        let snapped = round_to_step(value, field.step);
        field.value = format_value(snapped);
    }
}

pub(super) fn round_to_step(value: f32, step: f32) -> f32 {
    if step == 0.0 {
        return value;
    }
    (value / step).round() * step
}

pub(super) fn format_value(value: f32) -> String {
    format!("{value:.1}")
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
    field.value = format_value(snapped);
    Ok(snapped)
}

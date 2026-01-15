use crate::config::Target;
use crate::ptz_defs::{
    PAN_TILT_POSITION_SPACE, PAN_TILT_SPEED_SPACE, TT_NS, ZOOM_POSITION_SPACE, ZOOM_SPEED_SPACE,
};
use crate::ptz_parse::ServiceEndpoint;
use crate::soap;
use anyhow::{anyhow, Result};
use reqwest::Client;

pub async fn absolute_move(
    client: &Client,
    target: &Target,
    ptz: &ServiceEndpoint,
    profile: &str,
    pan: f32,
    tilt: f32,
) -> Result<()> {
    let action = format!("{}/AbsoluteMove", ptz.namespace);
    let body = absolute_move_body(&ptz.namespace, profile, pan, tilt);
    let response = soap::send(client, target, &ptz.xaddr, &action, &body, "").await?;
    if response.status >= 400 {
        return Err(anyhow!(
            "absolute move failed with HTTP {}: {}",
            response.status,
            response.body
        ));
    }
    Ok(())
}

fn absolute_move_body(namespace: &str, profile: &str, pan: f32, tilt: f32) -> String {
    let pan = format_float(pan);
    let tilt = format_float(tilt);
    format!(
        r#"<AbsoluteMove xmlns="{namespace}">
  <ProfileToken>{profile}</ProfileToken>
  <Position>
    <PanTilt x="{pan}" y="{tilt}" xmlns="{tt_ns}" space="{pan_pos}"></PanTilt>
    <Zoom x="0.0" xmlns="{tt_ns}" space="{zoom_pos}"></Zoom>
  </Position>
  <Speed>
    <PanTilt x="1" y="1" xmlns="{tt_ns}" space="{pan_speed}"/>
    <Zoom x="1.0" xmlns="{tt_ns}" space="{zoom_speed}"/>
  </Speed>
</AbsoluteMove>"#,
        namespace = namespace,
        profile = profile,
        pan = pan,
        tilt = tilt,
        tt_ns = TT_NS,
        pan_pos = PAN_TILT_POSITION_SPACE,
        zoom_pos = ZOOM_POSITION_SPACE,
        pan_speed = PAN_TILT_SPEED_SPACE,
        zoom_speed = ZOOM_SPEED_SPACE
    )
}

fn format_float(value: f32) -> String {
    let normalized = if value.abs() < f32::EPSILON { 0.0 } else { value };
    format!("{:.3}", normalized)
}

use crate::config::Target;
use crate::http_client;
use crate::ptz_control;
use crate::ptz_defs::{PTZ_PAN_MAX, PTZ_PAN_MIN, PTZ_PAN_STEP, PTZ_TILT_MAX, PTZ_TILT_MIN, PTZ_TILT_STEP};
use crate::ptz_input::{self, Command, Direction};
use crate::ptz_service;
use anyhow::{Context, Result};
use tokio::sync::mpsc;
pub async fn interactive_control(target: &Target) -> Result<()> {
    let client = http_client::build(target).context("ptz client build failed")?;
    let services = ptz_service::discover_services(&client, target).await?;
    let profile = ptz_service::get_profile_token(&client, target, &services.media).await?;
    println!("Media endpoint: {}", services.media.xaddr);
    println!("PTZ endpoint: {}", services.ptz.xaddr);
    println!("PTZ control: arrow keys to move, space to center, q/esc/ctrl+c to quit");
    println!("Profile token: {}", profile);
    let (tx, mut rx) = mpsc::channel(32);
    ptz_input::spawn_input_loop(tx);
    let mut position = Position::new();
    while let Some(command) = rx.recv().await {
        match command {
            Command::Move(dir) => {
                position.apply(dir);
                if let Err(err) = ptz_control::absolute_move(
                    &client,
                    target,
                    &services.ptz,
                    &profile,
                    position.pan,
                    position.tilt,
                )
                .await
                {
                    eprintln!("PTZ move failed:\n{:#}", err);
                }
            }
            Command::Center => {
                position.center();
                if let Err(err) = ptz_control::absolute_move(
                    &client,
                    target,
                    &services.ptz,
                    &profile,
                    position.pan,
                    position.tilt,
                )
                .await
                {
                    eprintln!("PTZ move failed:\n{:#}", err);
                }
            }
            Command::Quit => {
                break;
            }
        }
    }
    Ok(())
}
struct Position {
    pan: f32,
    tilt: f32,
}
impl Position {
    fn new() -> Self {
        Self { pan: 0.0, tilt: 0.0 }
    }
    fn apply(&mut self, dir: Direction) {
        match dir {
            Direction::Left => self.pan -= PTZ_PAN_STEP,
            Direction::Right => self.pan += PTZ_PAN_STEP,
            Direction::Up => self.tilt += PTZ_TILT_STEP,
            Direction::Down => self.tilt -= PTZ_TILT_STEP,
        }
        self.pan = self.pan.clamp(PTZ_PAN_MIN, PTZ_PAN_MAX);
        self.tilt = self.tilt.clamp(PTZ_TILT_MIN, PTZ_TILT_MAX);
    }
    fn center(&mut self) {
        self.pan = 0.0;
        self.tilt = 0.0;
    }
}

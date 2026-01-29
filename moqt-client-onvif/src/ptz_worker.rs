use crate::app_config::Target;
use crate::onvif_client::OnvifClient;
use crate::onvif_requests;
use crate::ptz_config::PtzRange;
use crate::ptz_state::PtzState;
use crate::soap_client;
use anyhow::{anyhow, Result};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
#[derive(Clone, Copy)]
pub enum Command {
    Absolute {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Relative {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Continuous {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Stop,
    Center {
        speed: f32,
    },
}
pub struct Controller {
    tx: Sender<Command>,
    err_rx: Receiver<String>,
    state_rx: Receiver<PtzState>,
}
impl Controller {
    pub fn new(target: Target) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        let (state_tx, state_rx) = mpsc::channel();
        thread::spawn(move || run_worker(target, rx, err_tx, state_tx));
        Ok(Self {
            tx,
            err_rx,
            state_rx,
        })
    }
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
    pub fn try_recv_error(&self) -> Option<String> {
        self.err_rx.try_recv().ok()
    }

    pub fn try_recv_state(&self) -> Option<PtzState> {
        self.state_rx.try_recv().ok()
    }
}
fn run_worker(
    target: Target,
    rx: Receiver<Command>,
    err_tx: Sender<String>,
    state_tx: Sender<PtzState>,
) {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => return send_init_error(&err_tx, format!("tokio runtime init failed: {err}")),
    };
    let client = match soap_client::build(&target) {
        Ok(client) => client,
        Err(err) => return send_init_error(&err_tx, err.to_string()),
    };
    let mut onvif = match runtime.block_on(OnvifClient::initialize(client, target)) {
        Ok(onvif) => onvif,
        Err(err) => return send_init_error(&err_tx, format!("ptz init error: {err}")),
    };
    let state = onvif.ptz_state();
    let _ = state_tx.send(state.clone());
    for message in onvif.take_gui_messages() {
        let _ = err_tx.send(message);
    }
    let range = state.range.clone();
    let mut position = Position::new(&range);
    let speed_range = range.speed_range();
    for command in rx {
        let result = match command {
            Command::Absolute {
                pan,
                tilt,
                zoom,
                speed,
            } => {
                position.set_absolute(pan, tilt, zoom, &range);
                let speed = speed_range.clamp(speed);
                let movement = onvif_requests::PtzMoveRequest {
                    pan: position.pan,
                    tilt: position.tilt,
                    zoom: position.zoom,
                    speed,
                };
                let cmd = onvif_requests::absolute_move(
                    onvif.profile_token(),
                    movement,
                    absolute_spaces(&range),
                );
                send_ptz_command(&runtime, &onvif, cmd)
            }
            Command::Relative {
                pan,
                tilt,
                zoom,
                speed,
            } => {
                let (pan_delta, tilt_delta, zoom_delta) =
                    position.apply_relative(pan, tilt, zoom, &range);
                let speed = speed_range.clamp(speed);
                let movement = onvif_requests::PtzMoveRequest {
                    pan: pan_delta,
                    tilt: tilt_delta,
                    zoom: zoom_delta,
                    speed,
                };
                let cmd = onvif_requests::relative_move(
                    onvif.profile_token(),
                    movement,
                    relative_spaces(&range),
                );
                send_ptz_command(&runtime, &onvif, cmd)
            }
            Command::Continuous {
                pan,
                tilt,
                zoom,
                speed,
            } => {
                let (pan_range, tilt_range) = range.continuous_pan_tilt_range();
                let zoom_range = range.continuous_zoom_range();
                let speed = speed_range.clamp(speed);
                let pan = pan_range.clamp(pan * speed);
                let tilt = tilt_range.clamp(tilt * speed);
                let zoom = zoom_range.clamp(zoom * speed);
                let velocity = onvif_requests::PtzVelocityRequest { pan, tilt, zoom };
                let cmd = onvif_requests::continuous_move(
                    onvif.profile_token(),
                    velocity,
                    continuous_spaces(&range),
                );
                send_ptz_command(&runtime, &onvif, cmd)
            }
            Command::Stop => {
                let cmd = onvif_requests::stop(onvif.profile_token(), true, true);
                send_ptz_command(&runtime, &onvif, cmd)
            }
            Command::Center { speed } => {
                position.center(&range);
                let speed = speed_range.clamp(speed);
                let movement = onvif_requests::PtzMoveRequest {
                    pan: position.pan,
                    tilt: position.tilt,
                    zoom: position.zoom,
                    speed,
                };
                let cmd = onvif_requests::absolute_move(
                    onvif.profile_token(),
                    movement,
                    absolute_spaces(&range),
                );
                send_ptz_command(&runtime, &onvif, cmd)
            }
        };
        if let Err(err) = result {
            let _ = err_tx.send(err.to_string());
        }
    }
}
fn send_init_error(err_tx: &Sender<String>, message: String) {
    let _ = err_tx.send(message);
}
fn send_ptz_command(
    runtime: &tokio::runtime::Runtime,
    onvif: &OnvifClient,
    command: onvif_requests::OnvifRequest,
) -> Result<()> {
    let operation = command.operation;
    let response = runtime.block_on(onvif.send_ptz(&command))?;
    if response.status >= 400 {
        soap_client::log_response(operation, onvif.ptz_endpoint(), &response);
        return Err(anyhow!("{operation} failed with HTTP {}", response.status));
    }
    Ok(())
}
struct Position {
    pan: f32,
    tilt: f32,
    zoom: f32,
}

fn absolute_spaces<'a>(range: &'a PtzRange) -> onvif_requests::PtzMoveSpaces<'a> {
    onvif_requests::PtzMoveSpaces {
        pan_tilt: range.absolute_pan_tilt_space(),
        zoom: range.absolute_zoom_space(),
        pan_tilt_speed: range.speed_pan_tilt_space(),
        zoom_speed: range.speed_zoom_space(),
    }
}

fn relative_spaces<'a>(range: &'a PtzRange) -> onvif_requests::PtzMoveSpaces<'a> {
    onvif_requests::PtzMoveSpaces {
        pan_tilt: range.relative_pan_tilt_space(),
        zoom: range.relative_zoom_space(),
        pan_tilt_speed: range.speed_pan_tilt_space(),
        zoom_speed: range.speed_zoom_space(),
    }
}

fn continuous_spaces<'a>(range: &'a PtzRange) -> onvif_requests::PtzVelocitySpaces<'a> {
    onvif_requests::PtzVelocitySpaces {
        pan_tilt: range.continuous_pan_tilt_space(),
        zoom: range.continuous_zoom_space(),
    }
}
impl Position {
    fn new(range: &PtzRange) -> Self {
        let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
        let zoom_range = range.absolute_zoom_range();
        Self {
            pan: pan_range.clamp(0.0),
            tilt: tilt_range.clamp(0.0),
            zoom: zoom_range.clamp(0.0),
        }
    }
    fn set_absolute(&mut self, pan: f32, tilt: f32, zoom: f32, range: &PtzRange) {
        let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
        let zoom_range = range.absolute_zoom_range();
        self.pan = pan_range.clamp(pan);
        self.tilt = tilt_range.clamp(tilt);
        self.zoom = zoom_range.clamp(zoom);
    }

    fn apply_relative(
        &mut self,
        pan: f32,
        tilt: f32,
        zoom: f32,
        range: &PtzRange,
    ) -> (f32, f32, f32) {
        let (pan_range, tilt_range) = range.relative_pan_tilt_range();
        let zoom_range = range.relative_zoom_range();
        let pan_delta = pan_range.clamp(pan);
        let tilt_delta = tilt_range.clamp(tilt);
        let zoom_delta = zoom_range.clamp(zoom);
        let (abs_pan_range, abs_tilt_range) = range.absolute_pan_tilt_range();
        let abs_zoom_range = range.absolute_zoom_range();
        self.pan = abs_pan_range.clamp(self.pan + pan_delta);
        self.tilt = abs_tilt_range.clamp(self.tilt + tilt_delta);
        self.zoom = abs_zoom_range.clamp(self.zoom + zoom_delta);
        (pan_delta, tilt_delta, zoom_delta)
    }
    fn center(&mut self, range: &PtzRange) {
        let (pan_range, tilt_range) = range.absolute_pan_tilt_range();
        let zoom_range = range.absolute_zoom_range();
        self.pan = pan_range.clamp(0.0);
        self.tilt = tilt_range.clamp(0.0);
        self.zoom = zoom_range.clamp(self.zoom);
    }
}

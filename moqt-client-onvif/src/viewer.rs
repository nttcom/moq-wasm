use crate::config::Target;
use crate::ptz_panel::Controls;
use crate::ptz_worker::Controller;
use crate::rtsp_worker::Stream;
use crate::video_view::VideoView;
use anyhow::{anyhow, Result};
use eframe::egui;

pub fn run(target: Target) -> Result<()> {
    let controller = Controller::new(target.clone())?;
    let video = Stream::new(target)?;
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "moqt-client-onvif PTZ",
        options,
        Box::new(|_| Box::new(PtzApp::new(controller, video))),
    )
    .map_err(|err| anyhow!("failed to start GUI: {err}"))?;
    Ok(())
}

struct PtzApp {
    controller: Controller,
    ptz_error: Option<String>,
    video: VideoView,
    controls: Controls,
}

impl PtzApp {
    fn new(controller: Controller, video: Stream) -> Self {
        Self {
            controller,
            ptz_error: None,
            video: VideoView::new(video),
            controls: Controls::new(),
        }
    }
}

impl eframe::App for PtzApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(err) = self.controller.try_recv_error() {
            self.ptz_error = Some(err);
        }
        if let Some(range) = self.controller.try_recv_range() {
            self.controls.set_range(range);
        }
        if let Some(node) = self.controller.try_recv_node() {
            self.controls.set_node_info(node);
        }
        self.video.update(ctx);
        egui::TopBottomPanel::bottom("ptz_controls")
            .resizable(false)
            .show(ctx, |ui| {
                if let Some(command) = self.controls.ui(ui) {
                    self.controller.send(command);
                }
                if let Some(err) = &self.ptz_error {
                    ui.add_space(8.0);
                    ui.label(format!("PTZ error: {err}"));
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            self.video.show(ui);
        });
    }
}

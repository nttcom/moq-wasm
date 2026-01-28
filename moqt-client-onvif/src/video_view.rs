use crate::rtsp_worker::Stream;
use eframe::egui;

pub struct VideoView {
    stream: Stream,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
}

impl VideoView {
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            texture: None,
            error: None,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        if let Some(err) = self.stream.try_recv_error() {
            self.error = Some(err);
        }
        if let Some(frame) = self.stream.recv_latest() {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.data);
            let options = egui::TextureOptions::LINEAR;
            match &mut self.texture {
                Some(texture) => texture.set(image, options),
                None => self.texture = Some(ctx.load_texture("rtsp", image, options)),
            }
            ctx.request_repaint();
        }
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        if let Some(texture) = &self.texture {
            let mut size = ui.available_size();
            if size.x <= 0.0 || size.y <= 0.0 {
                size = egui::Vec2::splat(1.0);
            }
            ui.add(egui::Image::from_texture(texture).fit_to_exact_size(size));
        } else {
            ui.label("Waiting for RTSP video...");
        }
        if let Some(err) = &self.error {
            ui.add_space(8.0);
            ui.label(format!("Video error: {err}"));
        }
    }
}

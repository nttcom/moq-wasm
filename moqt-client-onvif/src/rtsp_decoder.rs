use crate::app_config::Target;
use crate::rtsp_frame::Frame;
use ffmpeg_next as ffmpeg;
use std::sync::mpsc::Sender;
pub fn run(target: Target, tx: Sender<Frame>, err_tx: Sender<String>) {
    if let Err(err) = ffmpeg::init() {
        send_error(&err_tx, format!("ffmpeg init failed: {err}"));
        return;
    }
    let url = target.rtsp_url();
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("rtsp_transport", "tcp");
    let mut input = match ffmpeg::format::input_with_dictionary(&url, opts) {
        Ok(input) => input,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let stream = match input.streams().best(ffmpeg::media::Type::Video) {
        Some(stream) => stream,
        None => {
            send_error(&err_tx, "no video stream found".to_string());
            return;
        }
    };
    let stream_index = stream.index();
    let context = match ffmpeg::codec::context::Context::from_parameters(stream.parameters()) {
        Ok(context) => context,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut decoder = match context.decoder().video() {
        Ok(decoder) => decoder,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut scaler = match ffmpeg::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGBA,
        decoder.width(),
        decoder.height(),
        ffmpeg::software::scaling::Flags::BILINEAR,
    ) {
        Ok(scaler) => scaler,
        Err(err) => {
            send_error(&err_tx, err.to_string());
            return;
        }
    };
    let mut decoded = ffmpeg::util::frame::Video::empty();
    let mut rgba = ffmpeg::util::frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut decoded).is_ok() {
            if scaler.run(&decoded, &mut rgba).is_err() {
                continue;
            }
            if let Some(frame) = copy_frame(&rgba, decoder.width(), decoder.height()) {
                let _ = tx.send(frame);
            }
        }
    }
}
fn copy_frame(frame: &ffmpeg::util::frame::Video, width: u32, height: u32) -> Option<Frame> {
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let stride = frame.stride(0);
    let data = frame.data(0);
    if data.len() < stride * height {
        return None;
    }
    let mut out = vec![0u8; width * height * 4];
    for row in 0..height {
        let src_start = row * stride;
        let src_end = src_start + width * 4;
        let dst_start = row * width * 4;
        let dst_end = dst_start + width * 4;
        out[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
    }
    Some(Frame {
        width,
        height,
        data: out,
    })
}
fn send_error(err_tx: &Sender<String>, message: String) {
    let _ = err_tx.send(message);
}

use crate::app_config::Target;
use crate::rtsp_decoder;
use crate::rtsp_frame::Frame;
use anyhow::Result;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub struct Stream {
    rx: Receiver<Frame>,
    err_rx: Receiver<String>,
}

impl Stream {
    pub fn new(target: Target) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        thread::spawn(move || rtsp_decoder::run(target, tx, err_tx));
        Ok(Self { rx, err_rx })
    }

    pub fn recv_latest(&self) -> Option<Frame> {
        let mut last = None;
        while let Ok(frame) = self.rx.try_recv() {
            last = Some(frame);
        }
        last
    }

    pub fn try_recv_error(&self) -> Option<String> {
        self.err_rx.try_recv().ok()
    }
}

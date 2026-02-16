use bytes::Bytes;

use crate::{
    initializer::GStreamerInitializer, receiver::GStreamerReceiver, sender::GStreamerSender,
};

mod client;
mod initializer;
mod receiver;
mod sender;
mod stream_runner;

#[cfg(target_os = "macos")]
#[link(name = "foundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRun();
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopStop(rl: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
async fn start() {
    tracing::info!("GStreamer client sample started.");

    tokio::task::spawn(async {
        GStreamerInitializer::initialize().unwrap();
        
        let (sender_tx, receiver_rx) = tokio::sync::mpsc::channel::<(u64, Bytes)>(10);
        let sender = GStreamerSender::new(sender_tx).unwrap();
        tracing::info!("Starting GStreamer sender...");
        sender.start().unwrap();

        let receiver = GStreamerReceiver::new(receiver_rx).unwrap();
        receiver.start().unwrap();
        tracing::info!("To exit, press Ctrl+C in the terminal.");
        tokio::signal::ctrl_c().await.unwrap();
    });

    tokio::task::spawn(async {
        tracing::info!("To exit, press Ctrl+C in the terminal.");
        tokio::signal::ctrl_c().await.unwrap();
        unsafe {
            let main_loop = CFRunLoopGetMain();
            CFRunLoopStop(main_loop);
        }
    });

    unsafe { CFRunLoopRun() };
    tracing::info!("GStreamer client sample stopped.");
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    #[cfg(target_os = "macos")]
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(start());
    }
}

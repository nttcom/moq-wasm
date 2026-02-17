mod gstreamer_receiver;
mod message_receive_thread;
mod moqt_client;

#[cfg(target_os = "macos")]
#[link(name = "foundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRun();
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopStop(rl: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
pub(crate) async fn start() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    tokio::task::spawn(async {
        let cert_path = "/Users/gazzy/Project/moq-wasm/keys/cert.pem";
        let (sender, mut receiver) = tokio::sync::mpsc::channel(100);
        let moqt_client = moqt_client::MOQTClient::new(cert_path, sender)
            .await
            .expect("Failed to create MOQT client");
        moqt_client
            .subscribe_namespace("school")
            .await
            .expect("Failed to subscribe namespace");

        tracing::info!("waiting to get track namespace from message receive thread...");
        let track_namespace = receiver
            .recv()
            .await
            .expect("Failed to receive track namespace");
        let receiver = moqt_client
            .subscribe(&track_namespace, "grade1", moqt::SubscribeOption::default())
            .await
            .expect("Failed to subscribe");

        let gstreamer_receiver = gstreamer_receiver::GStreamerReceiver::new(receiver).unwrap();
        let _ = gstreamer_receiver.start();
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
    #[cfg(target_os = "macos")]
    {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            start().await;
        });
    }
}

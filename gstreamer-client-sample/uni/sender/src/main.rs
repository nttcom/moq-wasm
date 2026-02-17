use crate::{
    gstreamer_sender::GStreamerSender, media_send_thread::MediaSendThread, moqt_client::MOQTClient,
};

mod gstreamer_sender;
mod media_send_thread;
mod message_receive_thread;
mod moqt_client;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    let cert_path = "/Users/gazzy/Project/moq-wasm/keys/cert.pem";
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let (stream_sender, mut stream_receiver) =
            tokio::sync::mpsc::channel::<moqt::StreamDataSender<moqt::QUIC>>(100);
        let (data_sender, data_receiver) =
            tokio::sync::mpsc::channel::<gstreamer_sender::SendData>(100);

        let moqt_client = MOQTClient::new(cert_path, stream_sender).await.unwrap();
        moqt_client
            .publish_namespace("school/grade1")
            .await
            .unwrap();

        let gst_sender = GStreamerSender::new(data_sender).await;

        tracing::info!("waiting to get stream sender from message receive thread...");
        let stream = stream_receiver
            .recv()
            .await
            .expect("Failed to receive stream sender");

        let thread = MediaSendThread::start(data_receiver, stream);
        gst_sender.start();

        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C signal");
        drop(thread);
    });
}

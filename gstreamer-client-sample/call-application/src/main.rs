mod audio_receiver;
mod audio_sender;
mod media_send_thread;
mod message_receive_thread;
mod moqt_client;
mod video_receiver;
mod video_sender;

use crate::media_send_thread::MediaSendThread;
use crate::{audio_sender::AudioSender, video_sender::VideoSender};

#[cfg(not(feature = "use_datagram"))]
type StreamType = moqt::StreamDataSender<moqt::QUIC>;

#[cfg(feature = "use_datagram")]
type StreamType = moqt::DatagramSender<moqt::QUIC>;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 公開フラグ (--publish)
    #[arg(long, short)]
    publish: String,

    /// 購読フラグ (--subscribe)
    #[arg(long, short)]
    subscribe: String,
}

pub(crate) enum MOQTEvent {
    StreamAdded { is_video: bool, stream: StreamType },
    NamespaceAdded(String),
}

#[cfg(target_os = "macos")]
#[link(name = "foundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRun();
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopStop(rl: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
pub(crate) async fn start(publish_namespace: String, subscribe_namespace: String) {
    tokio::task::spawn(async move {
        use std::sync::Arc;

        use tokio::task::join_set;

        let cert_path = "/Users/gazzy/Project/moq-wasm/keys/cert.pem";
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel::<MOQTEvent>(100);
        let moqt_client = moqt_client::MOQTClient::new(cert_path, event_sender)
            .await
            .expect("Failed to create MOQT client");
        let moqt_client = Arc::new(moqt_client);
        moqt_client
            .publish_namespace(&publish_namespace)
            .await
            .expect("Failed to publish namespace");
        moqt_client
            .subscribe_namespace(&subscribe_namespace)
            .await
            .expect("Failed to subscribe namespace");

        tracing::info!("waiting to get track namespace from message receive thread...");
        let mut video_senders = vec![];
        let mut audio_senders = vec![];
        let mut media_send_threads = vec![];
        let mut join_set = join_set::JoinSet::new();
        loop {
            tokio::select! {
                Some(event) = event_receiver.recv() => {
                    match event {
                        MOQTEvent::StreamAdded { is_video, stream } => {
                            let (data_sender, data_receiver) =
                                tokio::sync::mpsc::channel::<video_sender::SendData>(1000);
                            if is_video {
                                let gst_sender = VideoSender::new(data_sender).await;
                                gst_sender.start();
                                video_senders.push(gst_sender);
                            } else {
                                let gst_sender = AudioSender::new(data_sender).await;
                                gst_sender.start();
                                audio_senders.push(gst_sender);
                            };
                            let media_send_thread = MediaSendThread::start(data_receiver, stream);
                            media_send_threads.push(media_send_thread);
                        }
                        MOQTEvent::NamespaceAdded(namespace) => {
                            let moqt_client = moqt_client.clone();
                            join_set.spawn(async move {
                                let video_receiver = moqt_client
                                .subscribe(&namespace, "video", moqt::SubscribeOption::default())
                                .await
                                .expect("Failed to subscribe");
                                let audio_receiver = moqt_client
                                    .subscribe(&namespace, "audio", moqt::SubscribeOption::default())
                                    .await
                                    .expect("Failed to subscribe");
                                let v_receiver = video_receiver::VideoReceiver::new(video_receiver).unwrap();
                                let a_receiver = audio_receiver::AudioReceiver::new(audio_receiver).unwrap();
                                let _ = a_receiver.start();
                                let _ = v_receiver.start();

                                tokio::signal::ctrl_c().await.unwrap();
                                let _ = v_receiver.stop();
                                let _ = a_receiver.stop();
                            });
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    join_set.abort_all();
                    break;
                }
            }
        }
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

    let args = Args::parse();
    let publish_namespace = args.publish;
    let subscribe_namespace = args.subscribe;
    tracing::info!("Publish Namespace: {}", publish_namespace);
    tracing::info!("Subscribe Namespace: {}", subscribe_namespace);

    #[cfg(target_os = "macos")]
    {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            start(publish_namespace, subscribe_namespace).await;
        });
    }
}

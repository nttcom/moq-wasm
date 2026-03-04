mod audio_receiver;
mod audio_sender;
mod media_send_thread;
mod message_receive_thread;
mod moqt_client;
mod video_receiver;
mod video_sender;

use crate::media_send_thread::MediaSendThread;
use crate::{audio_sender::AudioSender, video_sender::VideoSender};

// `use_datagram` フラグが **指定されていない** 時（＝デフォルト）
#[cfg(not(feature = "use_datagram"))]
type StreamType = moqt::StreamDataSender<moqt::QUIC>;

// `use_datagram` フラグが **指定されている** 時
#[cfg(feature = "use_datagram")]
type StreamType = moqt::DatagramSender<moqt::QUIC>;

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
pub(crate) async fn start() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    tokio::task::spawn(async {
        let cert_path = "/Users/gazzy/Project/moq-wasm/keys/cert.pem";
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel::<MOQTEvent>(100);
        let moqt_client = moqt_client::MOQTClient::new(cert_path, event_sender)
            .await
            .expect("Failed to create MOQT client");
        moqt_client
            .subscribe_namespace("school")
            .await
            .expect("Failed to subscribe namespace");

        tracing::info!("waiting to get track namespace from message receive thread...");
        let mut video_senders = vec![];
        let mut audio_senders = vec![];
        let mut video_receivers = vec![];
        let mut audio_receivers = vec![];
        loop {
            tokio::select! {
                Some(event) = event_receiver.recv() => {
                    match event {
                        MOQTEvent::StreamAdded { is_video, stream } => {
                            let (data_sender, data_receiver) =
                                tokio::sync::mpsc::channel::<video_sender::SendData>(100);
                            if is_video {
                                let gst_sender = VideoSender::new(data_sender).await;
                                gst_sender.start();
                                video_senders.push(gst_sender);
                            } else {
                                let gst_sender = AudioSender::new(data_sender).await;
                                gst_sender.start();
                                audio_senders.push(gst_sender);
                            };
                            MediaSendThread::start(data_receiver, stream);
                        }
                        MOQTEvent::NamespaceAdded(namespace) => {
                            let video_receiver = moqt_client
                                .subscribe(&namespace, "grade1", moqt::SubscribeOption::default())
                                .await
                                .expect("Failed to subscribe");
                            let audio_receiver = moqt_client
                                .subscribe(&namespace, "grade1", moqt::SubscribeOption::default())
                                .await
                                .expect("Failed to subscribe");
                            let v_receiver = video_receiver::VideoReceiver::new(video_receiver).unwrap();
                            let a_receiver = audio_receiver::AudioReceiver::new(audio_receiver).unwrap();
                            let _ = a_receiver.start();
                            let _ = v_receiver.start();
                            video_receivers.push(v_receiver);
                            audio_receivers.push(a_receiver);
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    for gst_receiver in video_receivers {
                        let _ = gst_receiver.stop();
                    }
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
    #[cfg(target_os = "macos")]
    {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            start().await;
        });
    }
}

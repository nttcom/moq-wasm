use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::datagram_receiver::DatagramReceiver,
    relay::ingest::received_event::ReceivedEvent,
    types::TrackKey,
};

pub(crate) struct DatagramReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn DatagramReceiver>,
}

pub(crate) struct DatagramReader {
    join_handle: JoinHandle<()>,
}

impl DatagramReader {
    pub(crate) fn run(
        mut rx: mpsc::Receiver<DatagramReceiveStart>,
        event_sender: mpsc::Sender<ReceivedEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(cmd) = rx.recv() => {
                        let track_key = cmd.track_key;
                        let _ = event_sender
                            .send(ReceivedEvent::DatagramOpened { track_key, group_id: 0 })
                            .await;
                        joinset.spawn(Self::read_loop(
                            track_key,
                            cmd.receiver,
                            event_sender.clone(),
                        ));
                    }
                    Some(result) = joinset.join_next() => {
                        if let Err(e) = result {
                            tracing::error!("datagram read task panicked: {:?}", e);
                        }
                    }
                    else => break,
                }
            }
        });
        Self { join_handle }
    }

    async fn read_loop(
        track_key: TrackKey,
        mut receiver: Box<dyn DatagramReceiver>,
        event_sender: mpsc::Sender<ReceivedEvent>,
    ) {
        let mut group_id = 0u64;
        loop {
            match receiver.receive_object().await {
                Ok(object) => {
                    let new_group_id = object.group_id().unwrap_or(group_id);
                    if new_group_id != group_id {
                        group_id = new_group_id;
                        let ev = ReceivedEvent::DatagramOpened { track_key, group_id };
                        if event_sender.send(ev).await.is_err() {
                            return;
                        }
                    }
                    let event = ReceivedEvent::Object { track_key, group_id, object };
                    if event_sender.send(event).await.is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = event_sender
                        .send(ReceivedEvent::DatagramClosed { track_key })
                        .await;
                    return;
                }
            }
        }
    }
}

impl Drop for DatagramReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

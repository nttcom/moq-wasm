use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::ingest::received_event::ReceivedEvent,
    types::TrackKey,
};

pub(crate) struct StreamOpened {
    pub(crate) track_key: TrackKey,
    pub(crate) receiver: Box<dyn StreamReceiver>,
}

pub(crate) struct StreamReader {
    join_handle: JoinHandle<()>,
}

impl StreamReader {
    pub(crate) fn run(
        mut rx: mpsc::Receiver<StreamOpened>,
        event_sender: mpsc::Sender<ReceivedEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(cmd) = rx.recv() => {
                        joinset.spawn(Self::read_loop(
                            cmd.track_key,
                            cmd.receiver,
                            event_sender.clone(),
                        ));
                    }
                    Some(result) = joinset.join_next() => {
                        if let Err(e) = result {
                            tracing::error!("stream read task panicked: {:?}", e);
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
        mut receiver: Box<dyn StreamReceiver>,
        event_sender: mpsc::Sender<ReceivedEvent>,
    ) {
        let mut group_id = 0u64;
        loop {
            match receiver.receive_object().await {
                Ok(DataObject::SubgroupHeader(header)) => {
                    group_id = header.group_id;
                    let event = ReceivedEvent::StreamOpened {
                        track_key,
                        group_id,
                        object: DataObject::SubgroupHeader(header),
                    };
                    if event_sender.send(event).await.is_err() {
                        return;
                    }
                }
                Ok(object) => {
                    let event = ReceivedEvent::Object { track_key, group_id, object };
                    if event_sender.send(event).await.is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = event_sender
                        .send(ReceivedEvent::EndOfGroup { track_key, group_id })
                        .await;
                    return;
                }
            }
        }
    }
}

impl Drop for StreamReader {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

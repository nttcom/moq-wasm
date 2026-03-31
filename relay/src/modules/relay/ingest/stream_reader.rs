use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::ingest::received_event::ReceivedEvent,
    types::TrackKey,
};

pub(crate) struct StreamOpened {
    pub(crate) track_key: TrackKey,
    pub(crate) stream_receiver: Box<dyn StreamReceiver>,
}

pub(crate) struct StreamReader {
    join_handle: JoinHandle<()>,
}

impl StreamReader {
    pub(crate) fn run(
        mut receiver: tokio::sync::mpsc::Receiver<StreamOpened>,
        sender: mpsc::Sender<ReceivedEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => {
                        let track_key = command.track_key;
                        let stream_receiver = command.stream_receiver;
                        joinset.spawn(Self::loop_receive(track_key, stream_receiver, sender.clone()));
                    }
                    Some(_) = joinset.join_next() => {
                        tracing::debug!("stream receive task ended");
                    }
                }
            }
        });
        Self { join_handle }
    }

    async fn loop_receive(
        track_key: TrackKey,
        mut stream_receiver: Box<dyn StreamReceiver>,
        sender: mpsc::Sender<ReceivedEvent>,
    ) {
        let mut group_id = 0;
        while let Ok(object) = stream_receiver.receive_object().await {
            match object {
                DataObject::SubgroupHeader(_) => {
                    group_id = object.group_id().unwrap();
                    if sender
                        .send(ReceivedEvent::StreamOpened {
                            track_key,
                            group_id,
                            object,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                DataObject::SubgroupObject(_) => {
                    if sender
                        .send(ReceivedEvent::Object {
                            track_key,
                            group_id,
                            object,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                DataObject::ObjectDatagram(_) => {
                    unreachable!("stream should not receive datagram object");
                }
            }
        }
        tracing::debug!(track_key, group_id, "stream receiver ended");
        let _ = sender
            .send(ReceivedEvent::EndOfGroup {
                track_key,
                group_id,
            })
            .await;
    }
}

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::datagram_receiver::DatagramReceiver,
    relay::ingest::received_event::ReceivedEvent, types::TrackKey,
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
        mut receiver: tokio::sync::mpsc::Receiver<DatagramReceiveStart>,
        sender: mpsc::Sender<ReceivedEvent>,
        initial_group_id: Option<u64>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut join_set = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => {
                        let track_key = command.track_key;
                        let datagram_receiver = command.receiver;
                        sender
                            .send(ReceivedEvent::DatagramOpened { track_key })
                            .await
                            .ok();
                        join_set.spawn(Self::loop_receive(
                            datagram_receiver,
                            sender.clone(),
                            track_key,
                            initial_group_id,
                        ));
                    },
                    Some(_) = join_set.join_next() => {
                        tracing::debug!("datagram receive task ended");
                    }
                }
            }
        });
        Self { join_handle }
    }

    async fn loop_receive(
        mut datagram_receiver: Box<dyn DatagramReceiver>,
        sender: mpsc::Sender<ReceivedEvent>,
        track_key: TrackKey,
        mut current_group_id: Option<u64>,
    ) {
        while let Ok(object) = datagram_receiver.receive_object().await {
            let group_id = object.group_id().or(current_group_id);
            let Some(group_id) = group_id else {
                tracing::warn!(track_key, "datagram object without group_id");
                continue;
            };
            current_group_id = Some(group_id);
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
        tracing::debug!(track_key, "datagram receiver ended");
        let _ = sender
            .send(ReceivedEvent::DatagramClosed { track_key })
            .await;
    }
}

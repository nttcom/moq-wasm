use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::datagram_receiver::DatagramReceiver,
    relay::ingest::received_event::ReceivedEvent,
    types::TrackKey,
};

pub(crate) struct DatagramReader;

impl DatagramReader {
    pub(crate) fn spawn(
        track_key: TrackKey,
        mut receiver: Box<dyn DatagramReceiver>,
        sender: mpsc::Sender<ReceivedEvent>,
        initial_group_id: Option<u64>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut current_group_id = initial_group_id;
            loop {
                match receiver.receive_object().await {
                    Ok(object) => {
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
                    Err(error) => {
                        tracing::debug!(track_key, ?error, "datagram receiver ended");
                        let _ = sender.send(ReceivedEvent::DatagramClosed { track_key }).await;
                        return;
                    }
                }
            }
        })
    }
}
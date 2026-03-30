use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::stream_receiver::StreamReceiver,
    relay::ingest::received_event::ReceivedEvent, types::TrackKey,
};

pub(crate) struct StreamReader;

impl StreamReader {
    pub(crate) fn spawn(
        track_key: TrackKey,
        mut receiver: Box<dyn StreamReceiver>,
        sender: mpsc::Sender<ReceivedEvent>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let first_object = match receiver.receive_object().await {
                Ok(object) => object,
                Err(error) => {
                    tracing::debug!(
                        track_key,
                        ?error,
                        "stream receiver ended before first object"
                    );
                    return;
                }
            };

            let Some(group_id) = first_object.group_id() else {
                tracing::warn!(track_key, "stream first object has no group_id");
                return;
            };

            if sender
                .send(ReceivedEvent::StreamOpened {
                    track_key,
                    group_id,
                })
                .await
                .is_err()
            {
                return;
            }

            if sender
                .send(ReceivedEvent::Object {
                    track_key,
                    group_id,
                    object: first_object,
                })
                .await
                .is_err()
            {
                return;
            }

            loop {
                match receiver.receive_object().await {
                    Ok(object) => {
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
                        tracing::debug!(track_key, group_id, ?error, "stream receiver ended");
                        let _ = sender
                            .send(ReceivedEvent::EndOfGroup {
                                track_key,
                                group_id,
                            })
                            .await;
                        return;
                    }
                }
            }
        })
    }
}

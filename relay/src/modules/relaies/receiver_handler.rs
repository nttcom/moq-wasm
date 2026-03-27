use crate::modules::{
    core::data_receiver::{
        datagram_receiver::DatagramReceiver, receiver::DataReceiver,
        stream_receiver::StreamReceiver,
    },
    relaies::receiver_monitor::ReceivedData,
};

pub(crate) struct ReceiverHandler {
    join_handle: tokio::task::JoinHandle<()>,
}

impl ReceiverHandler {
    fn run(
        track_key: u128,
        mut receiver: tokio::sync::mpsc::Receiver<DataReceiver>,
        sender: tokio::sync::mpsc::Sender<ReceivedData>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let joinset = tokio::task::JoinSet::new();
            let mut current_group_id: u64 = 0;
            loop {
                tokio::select! {
                    Some(data_receiver) = receiver.recv() => {
                        match data_receiver {
                            DataReceiver::Stream(stream_receiver) => {
                                joinset.spawn(Self::handle_stream(track_key, stream_receiver, sender.clone()));
                            }
                            DataReceiver::Datagram(datagram_receiver) => {
                                joinset.spawn(async move {
                                    if let Some(group_id) = Self::handle_datagram(track_key, current_group_id, datagram_receiver, sender.clone()).await {
                                        current_group_id = group_id;
                                    }
                                });
                            }
                        }
                    }
                    Some(join_result) = joinset.join_next() => {
                        if let Err(e) = join_result {
                            tracing::error!("Task failed: {:?}", e);
                        }
                    }
                    else => break,
                }
            }
        });
        ReceiverHandler { join_handle }
    }

    async fn handle_stream(
        track_key: u128,
        mut data_receiver: Box<dyn StreamReceiver>,
        sender: tokio::sync::mpsc::Sender<ReceivedData>,
    ) {
        let data_object = data_receiver.receive_object().await;
        match data_object {
            Ok(object) => {
                let received_data = ReceivedData::Data {
                    track_key,
                    group_id: data_receiver.get_group_id(),
                    data_object: object,
                };
                sender.send(received_data).await.unwrap_or_else(|e| {
                    tracing::error!("Failed to send received data: {:?}", e);
                });
            }
            Err(_) => {
                let received_data = ReceivedData::EndOfStream { track_key };
                sender.send(received_data).await.unwrap_or_else(|e| {
                    tracing::error!("Failed to send end of stream: {:?}", e);
                });
            }
        }
    }

    async fn handle_datagram(
        track_key: u128,
        current_group_id: u64,
        mut datagram_receiver: Box<dyn DatagramReceiver>,
        sender: tokio::sync::mpsc::Sender<ReceivedData>,
    ) -> Option<u64> {
        let data_object = datagram_receiver.receive_object().await;
        match data_object {
            Ok(object) => {
                let group_id = object.group_id().unwrap_or(current_group_id);
                let received_data = ReceivedData::Data {
                    track_key,
                    group_id,
                    data_object: object,
                };
                sender.send(received_data).await.unwrap_or_else(|e| {
                    tracing::error!("Failed to send received datagram: {:?}", e);
                });
                Some(group_id)
            }
            Err(_) => {
                let received_data = ReceivedData::EndOfStream { track_key };
                sender.send(received_data).await.unwrap_or_else(|e| {
                    tracing::error!("Failed to send end of stream for datagram: {:?}", e);
                });
                None
            }
        }
    }
}

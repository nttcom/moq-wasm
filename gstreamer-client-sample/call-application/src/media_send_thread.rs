use crate::{StreamType, video_sender::SendData};

pub(crate) struct MediaSendThread {
    join_handle: tokio::task::JoinHandle<()>,
}

impl MediaSendThread {
    pub(crate) fn start(
        receiver: tokio::sync::mpsc::Receiver<SendData>,
        stream_data_sender: StreamType,
    ) -> Self {
        #[cfg(not(feature = "use_datagram"))]
        {
            let join_handle = Self::handle_stream(receiver, stream_data_sender);
            MediaSendThread { join_handle }
        }
        #[cfg(feature = "use_datagram")]
        {
            let join_handle = Self::handle_datagram(receiver, stream_data_sender);
            MediaSendThread { join_handle }
        }
    }

    #[cfg(not(feature = "use_datagram"))]
    fn handle_stream(
        mut receiver: tokio::sync::mpsc::Receiver<SendData>,
        factory: StreamType,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn(async move {
            let mut join_set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
            let mut current_group_tx: Option<tokio::sync::mpsc::Sender<SendData>> = None;
            let mut current_group_id: Option<u64> = None;

            while let Some(data) = receiver.recv().await {
                // グループIDが変わったら旧グループのsenderをdropし新しいストリームをspawn
                if current_group_id != Some(data.group_id) {
                    drop(current_group_tx.take());
                    current_group_id = Some(data.group_id);

                    let mut stream = match factory.next().await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to open new stream: {}", e);
                            break;
                        }
                    };

                    let (tx, mut rx) = tokio::sync::mpsc::channel::<SendData>(32);
                    current_group_tx = Some(tx);

                    join_set.spawn(async move {
                        while let Some(item) = rx.recv().await {
                            let header = stream.create_header(
                                item.group_id,
                                moqt::SubgroupId::None,
                                128,
                                false,
                                false,
                            );
                            let obj = moqt::SubgroupObject::new_payload(item.payload);
                            let ex_header = moqt::ExtensionHeaders {
                                prior_group_id_gap: vec![],
                                prior_object_id_gap: vec![],
                                immutable_extensions: vec![],
                            };
                            let object_field =
                                stream.create_object_field(&header, item.object_id, ex_header, obj);
                            match stream.send(&header, object_field).await {
                                Ok(_) => {
                                    tracing::info!(
                                        "Sent data: group_id={}, object_id={}",
                                        item.group_id,
                                        item.object_id
                                    );
                                }
                                Err(e) => {
                                    tracing::error!("Failed to send data: {}", e);
                                    break;
                                }
                            }
                        }
                    });
                }

                if let Some(tx) = current_group_tx.as_ref() {
                    if let Err(e) = tx.send(data).await {
                        tracing::error!("Failed to forward data to group task: {}", e);
                        break;
                    }
                }
            }

            // 残グループタスクの完了を待つ
            join_set.join_all().await;
        })
    }

    #[cfg(feature = "use_datagram")]
    fn handle_datagram(
        mut receiver: tokio::sync::mpsc::Receiver<SendData>,
        mut stream_data_sender: StreamType,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn(async move {
            while let Some(data) = receiver.recv().await {
                use moqt::DatagramField;

                let datagram_field = DatagramField::Payload0x00 {
                    object_id: data.object_id,
                    publisher_priority: 128,
                    payload: data.payload,
                };
                let object =
                    stream_data_sender.create_object_datagram(data.group_id, datagram_field);

                match stream_data_sender.send(object).await {
                    Ok(_) => {
                        tracing::info!(
                            "Sent data: group_id={}, object_id={}",
                            data.group_id,
                            data.object_id
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to send data: {}", e);
                        break;
                    }
                }
            }
        })
    }
}

impl Drop for MediaSendThread {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

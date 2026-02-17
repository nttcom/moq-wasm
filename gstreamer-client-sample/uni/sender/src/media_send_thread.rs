use crate::gstreamer_sender::SendData;

pub(crate) struct MediaSendThread {
    join_handle: tokio::task::JoinHandle<()>,
}

impl MediaSendThread {
    pub(crate) fn start(
        mut receiver: tokio::sync::mpsc::Receiver<SendData>,
        mut stream_data_sender: moqt::StreamDataSender<moqt::QUIC>,
    ) -> Self {
        let join_handle = tokio::task::spawn(async move {
            while let Some(data) = receiver.recv().await {
                let header = stream_data_sender.create_header(
                    data.group_id,
                    moqt::SubgroupId::None,
                    128,
                    false,
                    false,
                );
                let obj = moqt::SubgroupObject::new_payload(data.payload);
                let ex_header = moqt::ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                };
                let object_field =
                    stream_data_sender.create_object_field(&header, data.object_id, ex_header, obj);
                match stream_data_sender.send(header, object_field).await {
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
        });
        MediaSendThread { join_handle }
    }
}

impl Drop for MediaSendThread {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

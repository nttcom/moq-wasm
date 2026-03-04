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
        mut stream_data_sender: StreamType,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn(async move {
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

use crate::modules::{
    moqt::{moqt_enums::ReceiveEvent, protocol::TransportProtocol}, transport::transport_receive_stream::TransportReceiveStream,
};

pub(crate) struct ControlReceiver {
    join_handle: tokio::task::JoinHandle<()>,
}

impl ControlReceiver {
    const RECEIVE_BYTES_CAPACITY: usize = 1024;

    pub(crate) fn new<T: TransportProtocol>(
        receive_stream: T::ReceiveStream,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> Self {
        let join_handle = Self::create_join_handle::<T>(receive_stream, sender);
        Self { join_handle }
    }

    fn create_join_handle<T: TransportProtocol>(
        mut receive_stream: T::ReceiveStream,
        sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("MOQTBiStream")
            .spawn(async move {
                let mut total_message = vec![];
                loop {
                    let mut bytes = vec![0u8; Self::RECEIVE_BYTES_CAPACITY];
                    let message = receive_stream.receive(&mut bytes).await;
                    if let Err(e) = message {
                        tracing::error!("failed to receive message: {:?}", e);
                        Self::disptach_receive_event(&sender, ReceiveEvent::Error());
                        break;
                    }
                    if let Some(size) = message.unwrap() {
                        tracing::debug!("Size {} message has been received", size);
                        total_message.extend_from_slice(&bytes[..size]);
                        if size == Self::RECEIVE_BYTES_CAPACITY {
                            tracing::debug!("Retry...");
                        } else {
                            tracing::debug!("message length: {}", total_message.len());
                            Self::disptach_receive_event(
                                &sender,
                                ReceiveEvent::Message(total_message.clone()),
                            );
                        }
                    } else {
                        tracing::debug!("message length: {}", total_message.len());
                        Self::disptach_receive_event(
                            &sender,
                            ReceiveEvent::Message(total_message.clone()),
                        );
                        total_message.clear();
                    }
                }
            })
            .unwrap()
    }

    fn disptach_receive_event(
        sender: &tokio::sync::broadcast::Sender<ReceiveEvent>,
        receive_event: ReceiveEvent,
    ) {
        loop {
            if sender.send(receive_event.clone()).is_ok() {
                tracing::debug!("Received message has been sent.");
                break;
            } else {
                tracing::warn!("Sending message failed. Retry.");
                continue;
            }
        }
    }
}

impl Drop for ControlReceiver {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

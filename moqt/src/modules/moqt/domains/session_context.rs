use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::{
            enums::{RequestId, ResponseMessage},
            threads::enums::StreamWithObject,
        },
        data_plane::streams::stream::bi_stream_sender::BiStreamSender,
    },
};

#[derive(Debug)]
pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    request_id: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map:
        tokio::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
    pub(crate) notification_map:
        tokio::sync::RwLock<HashMap<u64, tokio::sync::mpsc::UnboundedSender<StreamWithObject<T>>>>,
}

impl<T: TransportProtocol> SessionContext<T> {
    pub(crate) fn new(
        transport_connection: T::Connection,
        send_stream: BiStreamSender<T>,
        request_id: AtomicU64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> Self {
        Self {
            transport_connection,
            send_stream,
            request_id,
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
            notification_map: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn get_request_id(&self) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        tracing::debug!("request_id: {}", id);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }
}

impl<T: TransportProtocol> Drop for SessionContext<T> {
    fn drop(&mut self) {
        tracing::info!("Session dropped.");
        // send goaway
    }
}

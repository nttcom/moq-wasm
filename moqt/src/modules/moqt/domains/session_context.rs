use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        data_plane::stream::bi_stream_sender::BiStreamSender,
        runtime::dispatch::incoming_object::IncomingObject,
    },
};

pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    request_id: AtomicU64,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map:
        tokio::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject<T>>>>,
    incoming_object_routes: tokio::sync::Mutex<HashMap<u64, IncomingObjectRoute<T>>>,
}

enum IncomingObjectRoute<T: TransportProtocol> {
    Pending(VecDeque<IncomingObject<T>>),
    Ready(tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>),
}

pub(crate) enum IncomingObjectNotification {
    Notified,
    Buffered {
        pending_objects: usize,
        dropped_oldest: bool,
    },
    ReceiverClosed,
}

pub(crate) struct IncomingObjectReceiverRegistration {
    pub(crate) already_registered: bool,
    pub(crate) pending_objects: usize,
    pub(crate) failed_to_drain: bool,
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
            track_alias: AtomicU64::new(0),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            incoming_object_routes: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get_request_id(&self) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        tracing::debug!("request_id: {}", id);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }

    pub(crate) fn get_track_alias(&self) -> u64 {
        let track_alias = self.track_alias.fetch_add(1, Ordering::SeqCst);
        tracing::debug!("track_alias: {}", track_alias);
        track_alias
    }

    pub(crate) async fn notify_incoming_object(
        &self,
        track_alias: u64,
        incoming_object: IncomingObject<T>,
        max_pending_objects: usize,
    ) -> IncomingObjectNotification {
        let mut routes = self.incoming_object_routes.lock().await;
        match routes.entry(track_alias) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                IncomingObjectRoute::Ready(sender) => {
                    if sender.send(incoming_object).is_err() {
                        IncomingObjectNotification::ReceiverClosed
                    } else {
                        IncomingObjectNotification::Notified
                    }
                }
                IncomingObjectRoute::Pending(objects) => {
                    let dropped_oldest = objects.len() >= max_pending_objects;
                    if dropped_oldest {
                        objects.pop_front();
                    }
                    objects.push_back(incoming_object);
                    IncomingObjectNotification::Buffered {
                        pending_objects: objects.len(),
                        dropped_oldest,
                    }
                }
            },
            Entry::Vacant(entry) => {
                let mut objects = VecDeque::new();
                objects.push_back(incoming_object);
                entry.insert(IncomingObjectRoute::Pending(objects));
                IncomingObjectNotification::Buffered {
                    pending_objects: 1,
                    dropped_oldest: false,
                }
            }
        }
    }

    pub(crate) async fn register_incoming_object_receiver(
        &self,
        track_alias: u64,
        sender: tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>,
    ) -> IncomingObjectReceiverRegistration {
        let mut routes = self.incoming_object_routes.lock().await;
        match routes.remove(&track_alias) {
            Some(IncomingObjectRoute::Ready(existing_sender)) => {
                routes.insert(track_alias, IncomingObjectRoute::Ready(existing_sender));
                IncomingObjectReceiverRegistration {
                    already_registered: true,
                    pending_objects: 0,
                    failed_to_drain: false,
                }
            }
            Some(IncomingObjectRoute::Pending(mut pending_objects)) => {
                let pending_object_count = pending_objects.len();
                let mut failed_to_drain = false;
                while let Some(incoming_object) = pending_objects.pop_front() {
                    if sender.send(incoming_object).is_err() {
                        failed_to_drain = true;
                        break;
                    }
                }
                routes.insert(track_alias, IncomingObjectRoute::Ready(sender));
                IncomingObjectReceiverRegistration {
                    already_registered: false,
                    pending_objects: pending_object_count,
                    failed_to_drain,
                }
            }
            None => {
                routes.insert(track_alias, IncomingObjectRoute::Ready(sender));
                IncomingObjectReceiverRegistration {
                    already_registered: false,
                    pending_objects: 0,
                    failed_to_drain: false,
                }
            }
        }
    }
}

impl<T: TransportProtocol> fmt::Debug for SessionContext<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

impl<T: TransportProtocol> Drop for SessionContext<T> {
    fn drop(&mut self) {
        tracing::info!("SessionContext dropped.");
        // send goaway
    }
}

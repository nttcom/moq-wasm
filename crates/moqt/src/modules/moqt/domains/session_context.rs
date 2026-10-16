use std::{collections::HashMap, fmt, sync::Arc};

use crate::{
    SessionEvent,
    modules::moqt::{
        control_plane::{constants::TerminationErrorCode, enums::RequestId},
        data_plane::stream::fetch_data_receiver::FetchDataReceiver,
        domains::connection::{Connection, ObjectSink},
        runtime::dispatch::incoming_object::IncomingObject,
    },
};

/// The state of a session that outlives its transport connection: the
/// events, and the receivers the application takes per track alias and per
/// FETCH.
pub(crate) struct SessionContext {
    connection: Arc<Connection>,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject>>>,
    pub(crate) fetch_receiver_map:
        std::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<FetchDataReceiver>>>,
}

#[must_use = "dropping this removes the pending fetch; bind it for the lifetime of the request"]
pub(crate) struct PendingFetch {
    session: Arc<SessionContext>,
    connection: Arc<Connection>,
    request_id: RequestId,
    kept: bool,
}

impl PendingFetch {
    /// FETCH_OK arrived: the channel stays for `accept_fetch_receiver`.
    pub(crate) fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for PendingFetch {
    fn drop(&mut self) {
        if !self.kept {
            self.session
                .remove_pending_fetch(&self.connection, self.request_id);
        }
    }
}

impl SessionContext {
    pub(crate) fn new(
        connection: Connection,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            connection: Arc::new(connection),
            event_sender,
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            fetch_receiver_map: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The connection requests are sent on.
    pub(crate) fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// `receiver_map` is updated while the object-sinks lock is held, so the
    /// drain, the sink switch and the receiver registration are atomic.
    pub(crate) async fn register_data_receiver(
        &self,
        connection: &Connection,
        request_id: RequestId,
        track_alias: u64,
    ) -> Result<(), TerminationErrorCode> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject>();
        let mut sinks = connection.object_sinks.lock().await;

        if sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            return Err(TerminationErrorCode::DuplicateTrackAlias);
        }

        if let Some(ObjectSink::Buffer(mut pending_objects)) = sinks.remove(&track_alias) {
            while let Some(incoming_object) = pending_objects.pop_front() {
                let _ = sender.send(incoming_object);
            }
        }
        sinks.insert(track_alias, ObjectSink::Receiver { request_id, sender });

        self.receiver_map.lock().await.insert(track_alias, receiver);

        Ok(())
    }

    pub(crate) async fn cancel_subscription(&self, connection: &Connection, request_id: RequestId) {
        let mut sinks = connection.object_sinks.lock().await;
        let track_alias = sinks.iter().find_map(|(track_alias, sink)| {
            matches!(sink, ObjectSink::Receiver { request_id: id, .. } if *id == request_id)
                .then_some(*track_alias)
        });
        if let Some(track_alias) = track_alias {
            self.cancel_track_alias(&mut sinks, track_alias).await;
        }
    }

    pub(crate) async fn cancel_unclaimed_track_alias(
        &self,
        connection: &Connection,
        track_alias: u64,
    ) {
        let mut sinks = connection.object_sinks.lock().await;
        if !sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            self.cancel_track_alias(&mut sinks, track_alias).await;
        }
    }

    async fn cancel_track_alias(&self, sinks: &mut HashMap<u64, ObjectSink>, track_alias: u64) {
        sinks.insert(track_alias, ObjectSink::Cancelled);
        self.receiver_map.lock().await.remove(&track_alias);
    }

    /// Call before sending the FETCH on `connection`: its data stream can
    /// arrive before FETCH_OK.
    pub(crate) fn register_pending_fetch(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) -> PendingFetch {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<FetchDataReceiver>();
        connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .insert(request_id, sender);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .insert(request_id, receiver);
        PendingFetch {
            session: self.clone(),
            connection: connection.clone(),
            request_id,
            kept: false,
        }
    }

    pub(crate) fn remove_pending_fetch(&self, connection: &Connection, request_id: RequestId) {
        connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .remove(&request_id);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .remove(&request_id);
    }

    /// A close for a protocol violation is reported at once as
    /// `SessionEvent::ProtocolViolation`; any other close is reported as
    /// `SessionEvent::Disconnected` once the transport has closed, like a
    /// close by the peer.
    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        self.close_connection(&self.connection, code, reason);
    }

    /// Like `close_with_error`, for the connection a message arrived on.
    pub(crate) fn close_connection(
        &self,
        connection: &Connection,
        code: TerminationErrorCode,
        reason: &str,
    ) {
        if code == TerminationErrorCode::ProtocolViolation
            && let Err(error) = self.event_sender.send(SessionEvent::ProtocolViolation())
        {
            tracing::error!(?error, "failed to send protocol violation event");
        }
        connection.close(code, reason);
    }
}

impl fmt::Debug for SessionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

impl Drop for SessionContext {
    fn drop(&mut self) {
        tracing::debug!("SessionContext dropped.");
        // send goaway
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::modules::moqt::domains::connection::IncomingObjectNotification;
    use crate::modules::{
        moqt::runtime::dispatch::incoming_object::IncomingObject,
        test_support::{
            datagram_object, notify_datagram, register_and_take_data_receiver,
            spawn_connected_dual_sessions, subscription,
        },
    };

    const REQUEST_ID: u64 = 1;
    const OTHER_REQUEST_ID: u64 = 3;
    const TRACK_ALIAS: u64 = 5;

    #[tokio::test]
    async fn concurrent_callers_get_distinct_request_ids() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("request-id-concurrent").await;
        let context = server.subscriber().session;

        // Act
        let request_ids: Vec<u64> = std::thread::scope(|scope| {
            let callers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..1_000)
                            .map(|_| context.connection().control.next_request_id())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            callers
                .into_iter()
                .flat_map(|caller| caller.join().unwrap())
                .collect()
        });

        // Assert
        let distinct: HashSet<u64> = request_ids.iter().copied().collect();
        assert_eq!(distinct.len(), request_ids.len());
    }

    #[tokio::test]
    async fn objects_buffered_before_registration_are_delivered_to_the_receiver() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-early-object").await;
        let context = server.subscriber().session;
        notify_datagram(&context, TRACK_ALIAS).await;

        // Act
        let mut receiver = register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

        // Assert
        let received = receiver.try_recv();
        assert!(
            matches!(&received, Ok(IncomingObject::Datagram(object)) if *object == datagram_object()),
            "{received:?}"
        );
    }

    #[tokio::test]
    async fn unsubscribe_drops_the_unaccepted_receiver_and_discards_later_objects() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-unsubscribe-unaccepted").await;
        let context = server.subscriber().session;
        context
            .register_data_receiver(context.connection(), REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();

        // Act
        server
            .subscriber()
            .unsubscribe(&subscription(REQUEST_ID, TRACK_ALIAS))
            .await
            .unwrap();
        let notification = notify_datagram(&context, TRACK_ALIAS).await;

        // Assert
        assert!(!context.receiver_map.lock().await.contains_key(&TRACK_ALIAS));
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
    }

    #[tokio::test]
    async fn unsubscribe_ends_the_accepted_receiver() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-unsubscribe-accepted").await;
        let context = server.subscriber().session;
        let mut receiver = register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

        // Act
        server
            .subscriber()
            .unsubscribe(&subscription(REQUEST_ID, TRACK_ALIAS))
            .await
            .unwrap();

        // Assert
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn track_alias_of_a_cancelled_subscription_can_be_reused() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-reuse-alias").await;
        let context = server.subscriber().session;
        context
            .register_data_receiver(context.connection(), REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();
        context
            .cancel_subscription(context.connection(), REQUEST_ID)
            .await;

        // Act
        let registration = context
            .register_data_receiver(context.connection(), OTHER_REQUEST_ID, TRACK_ALIAS)
            .await;

        // Assert
        assert!(registration.is_ok());
    }
}

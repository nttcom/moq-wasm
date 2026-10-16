use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Context;
use futures::future::join_all;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::Span;

use crate::{
    RequestFailure, SessionEvent, TerminationErrorCode,
    modules::{
        executor::{self, BoxFuture, JoinHandle},
        moqt::{
            control_plane::control_messages::messages::unsubscribe::Unsubscribe,
            domains::{
                connection::{Connection, SharedCutover, SinkCutover, SinkReceiver},
                session_context::{CarriedSubscription, SessionContext},
                session_creator::Reconnect,
                subscriber::{send_subscribe, send_subscribe_namespace},
            },
            runtime::tasks::connection_tasks::ConnectionTasks,
            version::{ControlReceiver, Notification},
        },
        transport::transport_connection::TransportClose,
    },
    subscribe::{FilterType, SubscribeOption},
};

/// How long the new connection may take to connect, and to accept the
/// requests sent again.
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Publishers move on the same GOAWAY as their subscribers, so a request the
/// new server rejects is sent again at this interval.
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// How long the previous connection waits for a subscription that has not
/// cut over, such as one that delivers nothing on the new connection.
const CUTOVER_LIMIT: Duration = Duration::from_secs(10);
/// How long after the last cutover the previous connection keeps delivering
/// the groups that started on it.
#[cfg(not(test))]
const PREVIOUS_CONNECTION_GRACE: Duration = Duration::from_secs(5);
#[cfg(test)]
const PREVIOUS_CONNECTION_GRACE: Duration = Duration::from_millis(300);

/// What a client session that follows GOAWAY needs to open its next
/// connection.
pub(crate) struct Follow {
    pub(crate) reconnect: Box<dyn Reconnect>,
    pub(crate) url: String,
    /// The events of the first connection.
    pub(crate) connection_events: mpsc::UnboundedReceiver<SessionEvent>,
}

/// Reads the events of the session's connections, forwards them to
/// `Session::receive_event` with logical ids, and moves the session to the
/// connection a GOAWAY names (draft-14 §3.5).
pub(crate) struct MigrationTask {
    join_handle: JoinHandle,
}

impl MigrationTask {
    pub(crate) fn run(
        session: Arc<SessionContext>,
        first_tasks: ConnectionTasks,
        follow: Follow,
        closed_sender: watch::Sender<Option<TransportClose>>,
        session_span: Span,
    ) -> Self {
        let current = Live {
            connection: session.connection(),
            events: follow.connection_events,
            _tasks: first_tasks,
        };
        let router = Router {
            session,
            reconnect: follow.reconnect,
            url: follow.url,
            closed_sender,
            session_span,
        };
        Self {
            join_handle: executor::spawn("Session Migration", router.route(current)),
        }
    }
}

impl Drop for MigrationTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

/// A connection the session reads.
struct Live {
    connection: Arc<Connection>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    _tasks: ConnectionTasks,
}

struct Previous {
    live: Live,
    /// Resolves `PREVIOUS_CONNECTION_GRACE` after every carried subscription
    /// cut over (waiting at most `CUTOVER_LIMIT`), or at once when none moved.
    released: BoxFuture<()>,
    cutovers: Vec<Arc<std::sync::Mutex<SharedCutover>>>,
}

struct Migrated {
    live: Live,
    url: String,
    released: BoxFuture<()>,
    cutovers: Vec<Arc<std::sync::Mutex<SharedCutover>>>,
}

struct Router {
    session: Arc<SessionContext>,
    reconnect: Box<dyn Reconnect>,
    url: String,
    closed_sender: watch::Sender<Option<TransportClose>>,
    session_span: Span,
}

impl Router {
    async fn route(mut self, mut current: Live) {
        let mut previous: Option<Previous> = None;
        let mut migration: Option<BoxFuture<anyhow::Result<Migrated>>> = None;
        let mut current_closed = false;
        loop {
            tokio::select! {
                event = current.events.recv(), if !current_closed => match event {
                    Some(SessionEvent::GoAway(go_away)) => {
                        if migration.is_none() {
                            let url = match go_away.new_session_uri() {
                                "" => self.url.clone(),
                                uri => uri.to_string(),
                            };
                            tracing::info!(%url, "GOAWAY received; migrating the session");
                            let (event_sender, events) = mpsc::unbounded_channel();
                            let connecting = self.reconnect.connect(
                                url.clone(),
                                current.connection.generation + 1,
                                event_sender,
                            );
                            migration = Some(Box::pin(migrate(
                                self.session.clone(),
                                connecting,
                                events,
                                url,
                                self.session_span.clone(),
                            )));
                        }
                    }
                    Some(SessionEvent::Disconnected()) | None => {
                        current_closed = true;
                        if migration.is_none() {
                            self.finish(&current).await;
                            return;
                        }
                    }
                    Some(SessionEvent::PublishDone(handler)) => {
                        let request_id = current.connection.logical_request_id(handler.request_id());
                        self.forward(SessionEvent::PublishDone(handler.with_request_id(request_id)));
                    }
                    Some(event) => self.forward(event),
                },
                signal = previous_signal(previous.as_mut()) => match signal {
                    PreviousSignal::Released => {
                        if let Some(previous) = previous.take() {
                            tracing::info!("closing the previous connection");
                            self.retire(previous);
                        }
                    }
                    PreviousSignal::Event(Some(SessionEvent::GoAway(_) | SessionEvent::ProtocolViolation())) => {}
                    PreviousSignal::Event(Some(SessionEvent::Disconnected()) | None) => {
                        if let Some(previous) = previous.take() {
                            tracing::info!("the previous connection was closed");
                            self.retire(previous);
                        }
                    }
                    PreviousSignal::Event(Some(SessionEvent::PublishDone(handler))) => {
                        let Some(moved_from) = previous.as_ref() else {
                            continue;
                        };
                        let request_id = moved_from.live.connection.logical_request_id(handler.request_id());
                        // PUBLISH_DONE of a subscription carried to the current
                        // connection only reports the draining server; drop it.
                        if current.connection.wire_request_id(request_id).is_none() {
                            self.forward(SessionEvent::PublishDone(handler.with_request_id(request_id)));
                        }
                    }
                    PreviousSignal::Event(Some(event)) => self.forward(event),
                },
                migrated = optional(migration.as_mut()) => {
                    migration = None;
                    match migrated {
                        Ok(migrated) => {
                            if let Some(older) = previous.take() {
                                self.retire(older);
                            }
                            self.url = migrated.url;
                            previous = Some(Previous {
                                live: std::mem::replace(&mut current, migrated.live),
                                released: migrated.released,
                                cutovers: migrated.cutovers,
                            });
                            current_closed = false;
                            tracing::info!("moved to the new connection");
                        }
                        Err(error) => {
                            tracing::warn!(error = format!("{error:#}"), "migration failed; staying on the current connection");
                            if current_closed {
                                self.finish(&current).await;
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    fn forward(&self, event: SessionEvent) {
        let _ = self.session.event_sender.send(event);
    }

    /// Closes the previous connection with NO_ERROR; a subscription that has
    /// not cut over takes its next group start from the current one.
    fn retire(&self, previous: Previous) {
        previous
            .live
            .connection
            .close(TerminationErrorCode::NoError, "");
        self.session.forget_previous(&previous.live.connection);
        for cutover in previous.cutovers {
            cutover
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .cutover
                .previous_ended();
        }
    }

    async fn finish(&self, current: &Live) {
        let close = current.connection.transport.closed().await;
        self.closed_sender.send_replace(Some(close));
        self.forward(SessionEvent::Disconnected());
    }
}

enum PreviousSignal {
    Event(Option<SessionEvent>),
    Released,
}

async fn previous_signal(previous: Option<&mut Previous>) -> PreviousSignal {
    let Some(previous) = previous else {
        return std::future::pending().await;
    };
    tokio::select! {
        event = previous.live.events.recv() => PreviousSignal::Event(event),
        () = &mut previous.released => PreviousSignal::Released,
    }
}

async fn optional<F: Future + Unpin>(future: Option<&mut F>) -> F::Output {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

/// Opens the next connection, sends the namespace and track subscriptions
/// there again and makes it the current one. A subscription the new server
/// does not accept stays on the previous connection; a namespace it does not
/// accept fails the migration.
async fn migrate(
    session: Arc<SessionContext>,
    connecting: BoxFuture<anyhow::Result<(Connection, ControlReceiver)>>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    url: String,
    session_span: Span,
) -> anyhow::Result<Migrated> {
    let (connection, control_receiver) = executor::timeout(MIGRATION_TIMEOUT, connecting)
        .await
        .map_err(|()| anyhow::anyhow!("connecting to {url} timed out"))?
        .with_context(|| format!("cannot connect to {url}"))?;
    let connection = Arc::new(connection);
    let tasks = ConnectionTasks::run(&session, &connection, control_receiver, &session_span);
    let deadline = executor::now() + MIGRATION_TIMEOUT;
    let carried = session
        .carried
        .as_ref()
        .context("a session that follows GOAWAY carries its requests")?;
    let mut carried = carried.lock().await;
    let moved_from = session.connection();
    for namespace in &carried.namespaces {
        retry_until(deadline, || {
            send_subscribe_namespace(&connection, namespace.clone())
        })
        .await
        .with_context(|| format!("SUBSCRIBE_NAMESPACE {namespace} on the new connection failed"))?;
    }
    let subscriptions: Vec<(u64, &CarriedSubscription)> = carried
        .subscriptions
        .iter()
        .map(|(request_id, subscription)| (*request_id, subscription))
        .collect();
    let carried_over = join_all(subscriptions.into_iter().map(|(request_id, subscription)| {
        carry_subscription(&moved_from, &connection, request_id, subscription, deadline)
    }))
    .await;
    let mut cut_overs = Vec::new();
    let mut cutovers = Vec::new();
    for (request_id, outcome) in carried_over {
        match outcome {
            Carried::Moved { cut_over, cutover } => {
                cut_overs.push(cut_over);
                cutovers.push(cutover);
            }
            Carried::Stays => {}
            Carried::Gone => {
                carried.subscriptions.remove(&request_id);
            }
        }
    }
    session.move_to(connection.clone());
    drop(carried);
    Ok(Migrated {
        live: Live {
            connection,
            events,
            _tasks: tasks,
        },
        url,
        released: Box::pin(async move {
            if !cut_overs.is_empty() {
                let _ = executor::timeout(CUTOVER_LIMIT, join_all(cut_overs)).await;
                executor::sleep(PREVIOUS_CONNECTION_GRACE).await;
            }
        }),
        cutovers,
    })
}

enum Carried {
    /// Subscribed on the new connection; both sinks share `cutover`.
    Moved {
        cut_over: oneshot::Receiver<()>,
        cutover: Arc<std::sync::Mutex<SharedCutover>>,
    },
    /// The new server did not accept it; it stays on the previous connection.
    Stays,
    /// It no longer has a receiver.
    Gone,
}

async fn carry_subscription(
    moved_from: &Connection,
    connection: &Connection,
    request_id: u64,
    subscription: &CarriedSubscription,
    deadline: Duration,
) -> (u64, Carried) {
    let Some(previous_request_id) = moved_from.wire_request_id(request_id) else {
        return (request_id, Carried::Gone);
    };
    let option = SubscribeOption {
        filter_type: FilterType::NextGroupStart,
        ..subscription.option.clone()
    };
    let subscribed = retry_until(deadline, || {
        send_subscribe(
            connection,
            &subscription.track_namespace,
            &subscription.track_name,
            &option,
        )
    })
    .await;
    let subscribe_ok = match subscribed {
        Ok(subscribe_ok) => subscribe_ok,
        Err(error) => {
            tracing::warn!(
                namespace = %subscription.track_namespace,
                track = %subscription.track_name,
                %error,
                "the new connection did not accept the subscription; it stays on the previous one"
            );
            return (request_id, Carried::Stays);
        }
    };
    let (cut_over, cut_over_receiver) = oneshot::channel();
    let Some((logical_alias, sender, cutover)) = moved_from
        .start_cutover(previous_request_id, cut_over)
        .await
    else {
        let _ = connection
            .control
            .notify(Notification::Unsubscribe(Unsubscribe {
                request_id: subscribe_ok.request_id,
            }))
            .await;
        return (request_id, Carried::Gone);
    };
    connection.carry(request_id, subscribe_ok.request_id);
    if let Err(code) = connection
        .install_receiver(
            subscribe_ok.track_alias,
            SinkReceiver {
                request_id: subscribe_ok.request_id,
                sender,
                logical_alias,
                newest_group: None,
                cutover: Some(SinkCutover {
                    shared: cutover.clone(),
                    previous: false,
                }),
            },
        )
        .await
    {
        tracing::error!(
            ?code,
            "SUBSCRIBE_OK reused an in-use track alias on the new connection"
        );
        return (request_id, Carried::Stays);
    }
    (
        request_id,
        Carried::Moved {
            cut_over: cut_over_receiver,
            cutover,
        },
    )
}

async fn retry_until<T, F, R>(deadline: Duration, mut request: F) -> Result<T, RequestFailure>
where
    F: FnMut() -> R,
    R: Future<Output = Result<T, RequestFailure>>,
{
    loop {
        match request().await {
            Err(RequestFailure::Rejected { .. }) if executor::now() + RETRY_INTERVAL < deadline => {
                executor::sleep(RETRY_INTERVAL).await;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        ClientConfig, Session, SessionEvent, Subscription, TerminationErrorCode, TrackReader,
        modules::test_support::{
            DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, dual_client_with_config, open_subgroup,
            send_payload, spawn_dual_server, subscribed_track_reader,
        },
        object::{ContentExists, StreamDataSenderFactory},
        publish::PublishDoneCode,
        subscribe::{FilterType, SubscribeOption},
        wire::SubgroupId,
    };

    const NO_EVENT_TIMEOUT: Duration = Duration::from_millis(500);

    use super::*;

    async fn connect(port: u16) -> Session {
        let client = dual_client_with_config(ClientConfig {
            port: 0,
            verify_certificate: false,
            authorization_token: None,
            follow_go_away: true,
        });
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            client
                .connect(&format!("moqt://127.0.0.1:{port}"))
                .await?
                .await
        })
        .await
        .unwrap()
        .unwrap()
    }

    /// Answers the next SUBSCRIBE: where to send its objects, the filter it
    /// asked for and the subscription as the server holds it.
    async fn serve_subscribe(
        server: &Session,
    ) -> (StreamDataSenderFactory, FilterType, Subscription) {
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE");
        };
        let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
        let subscription = handler.into_subscription(track_alias);
        (
            server.publisher().create_stream(&subscription),
            subscription.filter_type(),
            subscription,
        )
    }

    async fn subscribe(
        client: &Session,
        server: &Session,
    ) -> (Subscription, StreamDataSenderFactory, Subscription) {
        let mut subscriber = client.subscriber();
        let (subscription, (sender, _, served)) = tokio::join!(
            subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
            serve_subscribe(server)
        );
        (subscription.unwrap(), sender, served)
    }

    async fn next_payload(reader: &mut TrackReader) -> (u64, Bytes) {
        let object = tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.next_object())
            .await
            .expect("object within timeout")
            .unwrap()
            .expect("track still open");
        (object.group_id, object.payload)
    }

    fn payload(group_id: u64, payload: &'static [u8]) -> (u64, Bytes) {
        (group_id, Bytes::from_static(payload))
    }

    #[tokio::test]
    async fn go_away_moves_a_subscription_to_the_named_server_at_a_group_boundary() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("session-migration-a");
        let (port_b, accept_b) = spawn_dual_server("session-migration-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (subscription, sender_a, _) = subscribe(&client, &server_a).await;
        let mut group_1_a = open_subgroup(&sender_a, 1, SubgroupId::None).await;
        send_payload(&mut group_1_a, b"a1-0").await;
        let mut reader = subscribed_track_reader(&client, &subscription).await;
        assert_eq!(next_payload(&mut reader).await, payload(1, b"a1-0"));

        // Act
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        let (sender_b, filter_b, _) = serve_subscribe(&server_b).await;
        send_payload(&mut group_1_a, b"a1-1").await;
        let previous_tail = next_payload(&mut reader).await;
        let mut group_1_b = open_subgroup(&sender_b, 1, SubgroupId::None).await;
        send_payload(&mut group_1_b, b"b1-0").await;
        let mut group_2_b = open_subgroup(&sender_b, 2, SubgroupId::None).await;
        send_payload(&mut group_2_b, b"b2-0").await;
        let cutover = next_payload(&mut reader).await;
        send_payload(&mut group_1_a, b"a1-2").await;
        let late_previous_tail = next_payload(&mut reader).await;
        let mut group_2_a = open_subgroup(&sender_a, 2, SubgroupId::None).await;
        send_payload(&mut group_2_a, b"a2-0").await;
        send_payload(&mut group_2_b, b"b2-1").await;
        let after_cutover = next_payload(&mut reader).await;
        let previous_close = tokio::time::timeout(DISCONNECT_TIMEOUT, server_a.closed()).await;

        // Assert: B's group 1 and A's group 2 are dropped; A is closed with NO_ERROR
        assert_eq!(filter_b, FilterType::NextGroupStart);
        assert_eq!(previous_tail, payload(1, b"a1-1"));
        assert_eq!(cutover, payload(2, b"b2-0"));
        assert_eq!(late_previous_tail, payload(1, b"a1-2"));
        assert_eq!(after_cutover, payload(2, b"b2-1"));
        assert_eq!(
            previous_close.expect("previous connection closed").code,
            Some(0)
        );
    }

    #[tokio::test]
    async fn publish_done_keeps_the_ids_the_application_knows_and_drops_the_drained_one() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("session-migration-done-a");
        let (port_b, accept_b) = spawn_dual_server("session-migration-done-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (subscription, _, served_a) = subscribe(&client, &server_a).await;
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        let (_, _, served_b) = serve_subscribe(&server_b).await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Act
        server_a
            .publisher()
            .publish_done(&served_a, PublishDoneCode::GoingAway, 0, String::new())
            .await
            .unwrap();
        server_b
            .publisher()
            .publish_done(&served_b, PublishDoneCode::TrackEnded, 0, String::new())
            .await
            .unwrap();
        let event = tokio::time::timeout(HANDSHAKE_TIMEOUT, client.receive_event())
            .await
            .unwrap()
            .unwrap();

        // Assert: only B's PUBLISH_DONE arrives, under the first request id
        let SessionEvent::PublishDone(done) = event else {
            panic!("expected PUBLISH_DONE, got {event:?}");
        };
        assert_eq!(done.request_id(), subscription.request_id());
        assert_eq!(done.status_code(), PublishDoneCode::TrackEnded);
    }

    #[tokio::test]
    async fn go_away_subscribes_the_namespaces_again_and_forwards_the_new_events() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("session-migration-namespace-a");
        let (port_b, accept_b) = spawn_dual_server("session-migration-namespace-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let subscriber = client.subscriber();
        let (subscribed, ()) = tokio::join!(subscriber.subscribe_namespace("room".into()), async {
            let SessionEvent::SubscribeNameSpace(handler) = server_a.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_NAMESPACE");
            };
            handler.ok().await.unwrap();
        });
        subscribed.unwrap();

        // Act
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        let SessionEvent::SubscribeNameSpace(handler) = server_b.receive_event().await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE on the new connection");
        };
        handler.ok().await.unwrap();
        let _pending = server_b
            .publisher()
            .begin_publish_namespace("room/alice".into())
            .await
            .unwrap();
        let event = tokio::time::timeout(HANDSHAKE_TIMEOUT, client.receive_event())
            .await
            .unwrap()
            .unwrap();

        // Assert: the GOAWAY itself is not returned
        assert_eq!(handler.track_namespace_prefix, "room");
        let SessionEvent::PublishNamespace(announced) = event else {
            panic!("expected PUBLISH_NAMESPACE, got {event:?}");
        };
        assert_eq!(announced.track_namespace, "room/alice");
    }

    #[tokio::test]
    async fn a_go_away_to_an_unreachable_server_keeps_the_current_connection() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("session-migration-unreachable");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (subscription, sender_a, _) = subscribe(&client, &server_a).await;

        // Act
        server_a
            .go_away("ftp://relay.invalid".to_string())
            .await
            .unwrap();
        let mut group_1 = open_subgroup(&sender_a, 1, SubgroupId::None).await;
        send_payload(&mut group_1, b"a1-0").await;
        let mut reader = subscribed_track_reader(&client, &subscription).await;
        let object = next_payload(&mut reader).await;
        let still_open = tokio::time::timeout(NO_EVENT_TIMEOUT, client.closed()).await;
        server_a.close_with_error(TerminationErrorCode::GoawayTimeout, "drained");
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed()).await;
        let event = tokio::time::timeout(DISCONNECT_TIMEOUT, client.receive_event()).await;

        // Assert
        assert_eq!(object, payload(1, b"a1-0"));
        assert!(still_open.is_err(), "the session must stay open");
        assert_eq!(close.expect("closed").code, Some(0x10));
        assert!(matches!(
            event.unwrap().unwrap(),
            SessionEvent::Disconnected()
        ));
    }
}

use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Context;
use futures::future::join_all;
use tokio::sync::{mpsc, oneshot};

use crate::{
    RequestFailure, Session, SessionEvent, Subscription, TerminationErrorCode,
    modules::{
        executor::{self, BoxFuture, JoinHandle},
        moqt::domains::migrating_session::{
            Current, Shared, TrackRequest, migrating_track::TrackSource,
        },
    },
    subscribe::{FilterType, SubscribeOption},
};

/// How long the new session may take to connect, and to accept the
/// subscriptions made again.
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Publishers move on the same GOAWAY as their subscribers, so a SUBSCRIBE
/// the new server rejects is sent again at this interval.
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// How long the previous session waits for a track that has not cut over,
/// such as one that delivers nothing on the new session.
const CUTOVER_LIMIT: Duration = Duration::from_secs(10);
/// How long after the last cutover the previous session keeps delivering the
/// groups that started on it.
#[cfg(not(test))]
const PREVIOUS_SESSION_GRACE: Duration = Duration::from_secs(5);
#[cfg(test)]
const PREVIOUS_SESSION_GRACE: Duration = Duration::from_millis(300);

/// Reads the events of the current and the previous session, forwards them
/// to `MigratingSession::receive_event`, and runs the migration a GOAWAY asks
/// for.
pub(super) struct MigrationTask {
    join_handle: JoinHandle,
}

impl MigrationTask {
    pub(super) fn run(
        shared: Arc<Shared>,
        event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            join_handle: executor::spawn("Session Migration", follow(shared, event_sender)),
        }
    }
}

impl MigrationTask {
    pub(super) fn abort(&self) {
        self.join_handle.abort();
    }
}

impl Drop for MigrationTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

struct Previous {
    session: Arc<Session>,
    /// Resolves `PREVIOUS_SESSION_GRACE` after every track cut over, or after
    /// `CUTOVER_LIMIT`.
    released: BoxFuture<()>,
}

struct Migrated {
    session: Arc<Session>,
    released: BoxFuture<()>,
}

async fn follow(shared: Arc<Shared>, event_sender: mpsc::UnboundedSender<SessionEvent>) {
    let mut current = shared.current().session;
    let mut current_events = true;
    let mut previous: Option<Previous> = None;
    let mut previous_events = false;
    let mut migration: Option<BoxFuture<anyhow::Result<Migrated>>> = None;
    loop {
        tokio::select! {
            event = current.receive_event(), if current_events => match event {
                Ok(SessionEvent::GoAway(go_away)) if migration.is_none() => {
                    let url = match go_away.new_session_uri() {
                        "" => shared.current().url,
                        uri => uri.to_string(),
                    };
                    tracing::info!(%url, "GOAWAY received; migrating the session");
                    shared.migrating.send_replace(true);
                    migration = Some(Box::pin(migrate(shared.clone(), url)));
                }
                Ok(SessionEvent::GoAway(_) | SessionEvent::Disconnected()) => {}
                Ok(event) => {
                    let _ = event_sender.send(event);
                }
                Err(_) => current_events = false,
            },
            event = previous_event(previous.as_ref().map(|previous| previous.session.clone())), if previous_events => match event {
                Ok(SessionEvent::GoAway(_) | SessionEvent::Disconnected()) => {}
                Ok(event) => {
                    let _ = event_sender.send(event);
                }
                Err(_) => previous_events = false,
            },
            migrated = optional(migration.as_mut()) => {
                migration = None;
                match migrated {
                    Ok(migrated) => {
                        if let Some(older) = previous.take() {
                            older.session.close_with_error(TerminationErrorCode::NoError, "");
                        }
                        previous = Some(Previous {
                            session: std::mem::replace(&mut current, migrated.session),
                            released: migrated.released,
                        });
                        current_events = true;
                        previous_events = true;
                        tracing::info!("moved to the new session");
                    }
                    Err(error) => {
                        tracing::warn!(error = format!("{error:#}"), "migration failed; staying on the current session");
                    }
                }
                shared.migrating.send_replace(false);
            }
            () = optional(previous.as_mut().map(|previous| &mut previous.released)) => {
                if let Some(previous) = previous.take() {
                    tracing::info!("closing the previous session");
                    previous.session.close_with_error(TerminationErrorCode::NoError, "");
                }
                previous_events = false;
            }
            close = current.closed(), if migration.is_none() => {
                shared.closed.send_replace(Some(close));
                let _ = event_sender.send(SessionEvent::Disconnected());
                return;
            }
        }
    }
}

async fn previous_event(previous: Option<Arc<Session>>) -> anyhow::Result<SessionEvent> {
    match previous {
        Some(previous) => previous.receive_event().await,
        None => std::future::pending().await,
    }
}

async fn optional<F: Future + Unpin>(future: Option<&mut F>) -> F::Output {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

/// A track the new session does not accept stays on the previous session; a
/// namespace it does not accept fails the migration.
async fn migrate(shared: Arc<Shared>, url: String) -> anyhow::Result<Migrated> {
    let session = executor::timeout(MIGRATION_TIMEOUT, shared.connector.connect(url.clone()))
        .await
        .map_err(|()| anyhow::anyhow!("connecting to {url} timed out"))?
        .with_context(|| format!("cannot connect to {url}"))?;
    let session = Arc::new(session);
    let deadline = executor::now() + MIGRATION_TIMEOUT;
    let mut requests = shared.requests.lock().await;
    for namespace in &requests.namespaces {
        retry_until(deadline, || {
            let subscriber = session.subscriber();
            let namespace = namespace.clone();
            async move { subscriber.subscribe_namespace(namespace).await }
        })
        .await
        .with_context(|| format!("SUBSCRIBE_NAMESPACE {namespace} on the new session failed"))?;
    }
    requests
        .tracks
        .retain(|track| !track.source_sender.is_closed());
    let subscriptions = join_all(
        requests
            .tracks
            .iter()
            .map(|track| resubscribe(&session, track, deadline)),
    )
    .await;
    let mut cut_overs = Vec::new();
    for (track, subscription) in requests.tracks.iter().zip(subscriptions) {
        let Some(subscription) = subscription else {
            continue;
        };
        let (cut_over, cut_over_receiver) = oneshot::channel();
        let source = TrackSource {
            session: session.clone(),
            subscription,
            cut_over,
        };
        if track.source_sender.send(source).is_ok() {
            cut_overs.push(cut_over_receiver);
        }
    }
    shared.replace_current(Current {
        session: session.clone(),
        url,
    });
    Ok(Migrated {
        session,
        released: Box::pin(async move {
            let _ = executor::timeout(CUTOVER_LIMIT, join_all(cut_overs)).await;
            executor::sleep(PREVIOUS_SESSION_GRACE).await;
        }),
    })
}

async fn resubscribe(
    session: &Session,
    track: &TrackRequest,
    deadline: Duration,
) -> Option<Subscription> {
    let option = SubscribeOption {
        filter_type: FilterType::NextGroupStart,
        ..track.option.clone()
    };
    let subscribed = retry_until(deadline, || {
        let mut subscriber = session.subscriber();
        let (namespace, name, option) =
            (track.namespace.clone(), track.name.clone(), option.clone());
        async move { subscriber.subscribe(namespace, name, option).await }
    })
    .await;
    match subscribed {
        Ok(subscription) => Some(subscription),
        Err(error) => {
            tracing::warn!(
                namespace = %track.namespace,
                track = %track.name,
                %error,
                "the new session did not accept the track; it stays on the previous one"
            );
            None
        }
    }
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

use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use moqt::{
    client::{self, NamespaceWatch},
    executor::{self, JoinHandle},
};
use tokio::time::Instant;

use super::remote_broadcast::RemoteBroadcast;
use crate::Session;

type NamespaceFilter = Arc<dyn Fn(&[String]) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct WatchOptions {
    /// Takes only the namespaces it returns `true` for.
    pub filter: Option<NamespaceFilter>,
    /// `watch_broadcasts` returns once no namespace has been published for
    /// this long, counted from the start or from the last withdrawal.
    pub empty_timeout: Option<Duration>,
}

/// Spawn `handler` for each remote namespace published under `prefix` (including those already published, skipping this session's own).
/// Withdrawal aborts the task; republishing starts a new one; handler errors are logged.
///
/// Returns when the session ends or `options.empty_timeout` elapses, aborting all tasks on return/drop.
/// Fails if `SUBSCRIBE_NAMESPACE` is rejected.
pub async fn watch_broadcasts<H, F>(
    session: &Session,
    prefix: &[impl AsRef<str>],
    options: WatchOptions,
    handler: H,
) -> anyhow::Result<()>
where
    H: Fn(RemoteBroadcast) -> F,
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let client = session.client().clone();
    let watch = ClosingWatch(Some(client.watch_namespaces(prefix).await?));
    let mut running: HashMap<Vec<String>, HandlerTask> = HashMap::new();
    let empty_deadline = || {
        options
            .empty_timeout
            .map(|timeout| Instant::now() + timeout)
    };
    let mut empty_until = empty_deadline();
    loop {
        let next = watch.next();
        let event = match empty_until.filter(|_| running.is_empty()) {
            Some(deadline) => match tokio::time::timeout_at(deadline, next).await {
                Ok(event) => event,
                Err(_) => return Ok(()),
            },
            None => next.await,
        };
        let Some(event) = event else {
            return Ok(());
        };
        if skips(&client, &options, &event.namespace) {
            continue;
        }
        running.remove(&event.namespace);
        if event.active {
            let broadcast = RemoteBroadcast::new(client.clone(), event.namespace.clone());
            let task = HandlerTask::run(handler(broadcast), event.namespace.join("/"));
            running.insert(event.namespace, task);
        } else if running.is_empty() {
            empty_until = empty_deadline();
        }
    }
}

fn skips(client: &client::Session, options: &WatchOptions, namespace: &[String]) -> bool {
    client
        .moqt_session()
        .publishes_namespace(&namespace.join("/"))
        || options
            .filter
            .as_ref()
            .is_some_and(|filter| !filter(namespace))
}

/// One handler run, aborted when dropped.
struct HandlerTask {
    join_handle: JoinHandle,
}

impl HandlerTask {
    fn run(
        handler: impl Future<Output = anyhow::Result<()>> + Send + 'static,
        track_namespace: String,
    ) -> Self {
        let join_handle = executor::spawn("moq-wasm-broadcast-handler", async move {
            if let Err(error) = handler.await {
                tracing::warn!(error = %format!("{error:#}"), track_namespace, "broadcast handler failed");
            }
        });
        Self { join_handle }
    }
}

impl Drop for HandlerTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

/// UNSUBSCRIBE_NAMESPACEs on a spawned task when dropped.
struct ClosingWatch(Option<NamespaceWatch>);

impl ClosingWatch {
    async fn next(&self) -> Option<moqt::namespace::NamespaceEvent> {
        self.0.as_ref()?.next().await
    }
}

impl Drop for ClosingWatch {
    fn drop(&mut self) {
        let Some(watch) = self.0.take() else {
            return;
        };
        executor::spawn("moq-wasm-unsubscribe-namespace", async move {
            if let Err(error) = watch.close().await {
                tracing::debug!(error = %format!("{error:#}"), "UNSUBSCRIBE_NAMESPACE failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::test_support::{TEST_TIMEOUT, TestRelay};

    /// Sends `true` when the handler holding it is dropped, as when aborted.
    struct Dropped(mpsc::UnboundedSender<bool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.send(true);
        }
    }

    #[tokio::test]
    async fn a_handler_runs_for_each_namespace_but_the_sessions_own() {
        // Arrange
        let mut relay = TestRelay::spawn("watch-own");
        let session = Arc::new(relay.connect().await);
        let (namespace_sender, mut namespace_receiver) = mpsc::unbounded_channel();
        let watching = session.clone();
        let _watch = tokio::spawn(async move {
            watch_broadcasts(&watching, &["anon"], WatchOptions::default(), |broadcast| {
                let namespace_sender = namespace_sender.clone();
                async move {
                    let _ = namespace_sender.send(broadcast.namespace().join("/"));
                    Ok(())
                }
            })
            .await
        });
        assert_eq!(relay.next_request().await, "SUBSCRIBE_NAMESPACE anon");

        // Act
        let _own = session.announce(&["anon", "own"]).await.unwrap();
        relay.next_request().await;
        relay.publish_namespace("anon/remote").await;
        let first = tokio::time::timeout(TEST_TIMEOUT, namespace_receiver.recv()).await;
        let second =
            tokio::time::timeout(Duration::from_millis(200), namespace_receiver.recv()).await;

        // Assert: no handler ran for anon/own, which the relay echoed back
        assert_eq!(first.unwrap().as_deref(), Some("anon/remote"));
        assert!(second.is_err(), "{second:?}");
    }

    #[tokio::test]
    async fn a_withdrawal_aborts_the_handler_and_the_empty_timeout_ends_the_watch() {
        // Arrange
        let mut relay = TestRelay::spawn("watch-withdraw");
        let session = Arc::new(relay.connect().await);
        let (dropped_sender, mut dropped_receiver) = mpsc::unbounded_channel();
        let options = WatchOptions {
            empty_timeout: Some(Duration::from_millis(300)),
            ..WatchOptions::default()
        };
        let watching = session.clone();
        let watch = tokio::spawn(async move {
            watch_broadcasts(&watching, &["anon"], options, |_| {
                let dropped = Dropped(dropped_sender.clone());
                async move {
                    let _dropped = dropped;
                    std::future::pending::<()>().await;
                    Ok(())
                }
            })
            .await
        });
        relay.next_request().await;
        relay.publish_namespace("anon/remote").await;

        // Act
        relay.publish_namespace_done("anon/remote").await;
        let aborted = tokio::time::timeout(TEST_TIMEOUT, dropped_receiver.recv()).await;
        let ended = tokio::time::timeout(TEST_TIMEOUT, watch).await;

        // Assert
        assert_eq!(aborted.unwrap(), Some(true));
        assert!(ended.unwrap().unwrap().is_ok());
    }
}

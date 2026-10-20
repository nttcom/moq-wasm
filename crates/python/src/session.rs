use std::{sync::Arc, time::Duration};

use moqt::{
    ClientConfig, DUAL, Endpoint, RequestErrorCode, SessionEvent, TerminationErrorCode,
    executor::{self, JoinHandle},
    publish::{Origin, Retention},
};
use pyo3::prelude::*;

use crate::{
    namespaces::{NamespaceEvents, NamespaceWatchers},
    publication::TrackPublication,
    track::Track,
};

/// Connects to a relay: `moqt://` for raw QUIC, `https://` for WebTransport.
/// The session follows the relay's GOAWAY by itself.
#[pyfunction]
#[pyo3(signature = (url, *, verify_certificate = true))]
pub fn connect(
    py: Python<'_>,
    url: String,
    verify_certificate: bool,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // quinn binds the endpoint's socket through the ambient runtime, which this future runs on.
        let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
            port: 0,
            verify_certificate,
            authorization_token: None,
            follow_go_away: true,
        })?;
        let session = Arc::new(endpoint.connect(&url).await?.await?);
        let origin = Origin::default();
        let watchers = Arc::new(NamespaceWatchers::default());
        let event_loop = executor::spawn(
            "python-session-events",
            route_events(session.clone(), origin.clone(), watchers.clone()),
        );
        Ok(Session {
            session,
            endpoint: Arc::new(endpoint),
            origin,
            watchers,
            event_loop,
        })
    })
}

/// A session has one event receiver: the origin answers what concerns the
/// tracks this session publishes, and namespace events go to the iterators.
async fn route_events(
    session: Arc<moqt::Session>,
    origin: Origin,
    watchers: Arc<NamespaceWatchers>,
) {
    loop {
        let event = match session.receive_event().await {
            Ok(event) => event,
            Err(err) => {
                tracing::info!(?err, "session event loop ended");
                break;
            }
        };
        let Some(event) = origin.handle(event).await else {
            continue;
        };
        match event {
            SessionEvent::PublishNamespace(handler) => {
                if let Err(err) = handler.ok().await {
                    tracing::warn!(namespace = %handler.track_namespace, ?err, "failed to ack PUBLISH_NAMESPACE");
                }
                watchers.notify(handler.track_namespace_tuple.clone(), true);
            }
            SessionEvent::PublishNamespaceDone(handler) => {
                let namespace = handler
                    .track_namespace
                    .split('/')
                    .map(str::to_string)
                    .collect();
                watchers.notify(namespace, false);
            }
            SessionEvent::SubscribeNameSpace(handler) => {
                if let Err(err) = handler.ok().await {
                    tracing::warn!(prefix = %handler.track_namespace_prefix, ?err, "failed to ack SUBSCRIBE_NAMESPACE");
                }
            }
            SessionEvent::Subscribe(handler) => {
                let reason = format!("{} is not a track of this session", handler.track_name);
                if let Err(err) = handler
                    .error(RequestErrorCode::TrackDoesNotExist, reason)
                    .await
                {
                    tracing::warn!(?err, "failed to reject SUBSCRIBE");
                }
            }
            SessionEvent::Publish(handler) => {
                let reason = "this session takes no PUBLISH".to_string();
                if let Err(err) = handler.error(RequestErrorCode::Uninterested, reason).await {
                    tracing::warn!(?err, "failed to reject PUBLISH");
                }
            }
            SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
            event => tracing::debug!(?event, "unhandled control message"),
        }
    }
    watchers.close();
}

#[pyclass(module = "moqt")]
pub struct Session {
    session: Arc<moqt::Session>,
    endpoint: Arc<Endpoint<DUAL>>,
    origin: Origin,
    watchers: Arc<NamespaceWatchers>,
    event_loop: JoinHandle,
}

// The event loop holds the session, so a Session dropped without `close` would
// otherwise keep the connection open until the relay ends it.
impl Drop for Session {
    fn drop(&mut self) {
        self.event_loop.abort();
    }
}

#[pymethods]
impl Session {
    /// Iterates the namespaces published and withdrawn under `prefix`.
    fn subscribe_namespace<'py>(
        &self,
        py: Python<'py>,
        prefix: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        // Registered before the request, since the relay answers with the namespaces it knows at once.
        let events: NamespaceEvents = self.watchers.watch(prefix.clone());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .subscriber()
                .subscribe_namespace(prefix.join("/"))
                .await
                .map_err(anyhow::Error::from)?;
            Ok(events)
        })
    }

    #[pyo3(signature = (namespace, name, *, current_group = false))]
    fn subscribe<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        current_group: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            Track::subscribe(session, namespace.join("/"), name, current_group)
                .await
                .map_err(PyErr::from)
        })
    }

    fn publish_namespace<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .publisher()
                .publish_namespace(namespace.join("/"))
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
    }

    /// Registers a track with the session's origin, which keeps what is written
    /// for `retention_seconds` to answer FETCH. Its group ids start at the
    /// wall-clock time in µs, so a later run of the same publisher does not
    /// reuse a location the relay still caches.
    #[pyo3(signature = (namespace, name, *, retention_seconds))]
    fn track<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        retention_seconds: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let origin = self.origin.clone();
        let namespace = namespace.join("/");
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let first_group_id = executor::system_time().as_micros() as u64;
            let retention = Retention::For(Duration::from_secs_f64(retention_seconds));
            origin
                .ensure_track(&namespace, &name, first_group_id, retention)
                .await;
            Ok(TrackPublication {
                origin,
                namespace,
                name,
            })
        })
    }

    /// Waits until the relay closes the session.
    fn closed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.closed().await;
            Ok(())
        })
    }

    /// Closes the session and waits until the relay has seen the close.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.event_loop.abort();
        self.watchers.close();
        let session = self.session.clone();
        let endpoint = self.endpoint.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.close_with_error(TerminationErrorCode::NoError, "");
            endpoint.wait_idle().await;
            Ok(())
        })
    }
}

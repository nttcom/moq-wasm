use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use moqt::{
    AuthorizationToken, AuthorizationTokenSource, ClientConfig, Endpoint, NATIVE, RequestErrorCode,
    SessionEvent,
    executor::{self, JoinHandle},
    namespace::NamespaceWatchers,
    publish::{Origin, Retention},
};
use pyo3::{exceptions::PyRuntimeError, prelude::*};

use crate::{namespaces::NamespaceEvents, publication::TrackPublication, track::Track};

/// The relay takes a TRACK_STATUS that carries an AUTHORIZATION TOKEN as a
/// token refresh; it reads neither the namespace `[app id]` nor this name.
const REFRESH_TRACK_NAME: &str = "update_auth_token";

/// Connects to a relay: `moqt://` for raw QUIC, `https://` for WebTransport.
/// The session follows the relay's GOAWAY by itself, presenting the latest
/// token given to `connect` or `refresh_token`.
#[pyfunction]
#[pyo3(signature = (url, *, verify_certificate = true, token = None))]
pub fn connect(
    py: Python<'_>,
    url: String,
    verify_certificate: bool,
    token: Option<String>,
) -> PyResult<Bound<'_, PyAny>> {
    let token = token.map(|token| Arc::new(Mutex::new(token)));
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let authorization_token = token.clone().map(|current| {
            AuthorizationTokenSource::provider(move || {
                let token = current.lock().unwrap().clone();
                async move { Ok(token) }
            })
        });
        // quinn binds the endpoint's socket through the ambient runtime, which this future runs on.
        let endpoint = Endpoint::<NATIVE>::create_client(&ClientConfig {
            verify_certificate,
            authorization_token,
            ..ClientConfig::default()
        })?;
        let session = Arc::new(endpoint.connect(&url).await?.await?);
        let origin = Origin::default();
        let watchers = NamespaceWatchers::default();
        let event_loop = executor::spawn(
            "python-session-events",
            route_events(session.clone(), origin.clone(), watchers.clone()),
        );
        Ok(Session {
            session,
            origin,
            watchers,
            event_loop,
            token,
        })
    })
}

/// A session has one event receiver: the origin answers what concerns the
/// tracks this session publishes, and namespace events go to the iterators.
async fn route_events(session: Arc<moqt::Session>, origin: Origin, watchers: NamespaceWatchers) {
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
        let Some(event) = watchers.handle(event).await else {
            continue;
        };
        match event {
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
    origin: Origin,
    watchers: NamespaceWatchers,
    event_loop: JoinHandle,
    /// The token the session presents when it follows a GOAWAY.
    token: Option<Arc<Mutex<String>>>,
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
        let events = NamespaceEvents::from(self.watchers.watch(prefix.clone()));
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

    /// Sends `token` to the relay as the session's new authorization token
    /// (docs/architecture/relay/architecture.md "Token refresh"), and presents
    /// it on the connections the session opens from now on. Fails for a
    /// session connected without a token, or when the relay rejects `token`;
    /// the relay then keeps the session's current token.
    #[pyo3(signature = (token, *, app_id))]
    fn refresh_token<'py>(
        &self,
        py: Python<'py>,
        token: String,
        app_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let Some(current) = self.token.clone() else {
            return Err(PyRuntimeError::new_err(
                "the session was connected without a token",
            ));
        };
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .subscriber()
                .track_status(
                    app_id,
                    REFRESH_TRACK_NAME.to_string(),
                    vec![AuthorizationToken::use_value_utf8(&token)],
                )
                .await
                .map_err(anyhow::Error::from)?;
            *current.lock().unwrap() = token;
            Ok(())
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
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.close().await;
            Ok(())
        })
    }
}

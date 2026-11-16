use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use moqt::{
    AuthorizationToken, AuthorizationTokenSource, CertificateVerification, ClientConfig, Endpoint,
    NATIVE, Version,
    client::{self, PublishTrackOptions, RequestError, SubscribeOptions},
    publish::Retention,
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};

use broadcast::{Broadcast, RelayConnection};

use crate::{
    media_broadcast::MediaBroadcast, namespace_watch::NamespaceWatch, subscription::Subscription,
    track_writer::TrackWriter,
};

/// The relay takes a TRACK_STATUS that carries an AUTHORIZATION TOKEN as a
/// token refresh; it reads neither the namespace `[app id]` nor this name.
const REFRESH_TRACK_NAME: &str = "update_auth_token";

/// Connects to a relay: `moqt://` for raw QUIC, `https://` for WebTransport.
/// The session follows the relay's GOAWAY by itself, presenting the latest
/// token given to `connect` or `refresh_token`.
#[pyfunction]
#[pyo3(signature = (url, *, verify_certificate = true, token = None, versions = None))]
pub fn connect(
    py: Python<'_>,
    url: String,
    verify_certificate: bool,
    token: Option<String>,
    versions: Option<Vec<String>>,
) -> PyResult<Bound<'_, PyAny>> {
    let versions = Version::offered(versions.as_deref())
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
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
            certificate_verification: CertificateVerification::native_roots_or_disabled(
                verify_certificate,
            ),
            authorization_token,
            versions,
            ..ClientConfig::default()
        })?;
        let session = endpoint.connect(&url).await?.await?;
        Ok(Session {
            session: Arc::new(client::Session::new(session)),
            token,
        })
    })
}

/// The message of a failed request's Python exception: the `RequestFailure` alone, without the
/// `{kind:?} failed:` prefix `RequestError` displays.
fn request_error(error: RequestError) -> anyhow::Error {
    error.failure.into()
}

#[pyclass(module = "moq_wasm")]
pub struct Session {
    session: Arc<client::Session>,
    /// The token the session presents when it follows a GOAWAY.
    token: Option<Arc<Mutex<String>>>,
}

#[pymethods]
impl Session {
    /// SUBSCRIBE_NAMESPACE: iterates the namespaces published and withdrawn under `prefix`.
    fn watch_namespaces<'py>(
        &self,
        py: Python<'py>,
        prefix: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let watch = session
                .watch_namespaces(&prefix)
                .await
                .map_err(request_error)?;
            Ok(NamespaceWatch::from(watch))
        })
    }

    fn subscribe<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let subscription = session
                .subscribe(&namespace, &name, SubscribeOptions::default())
                .await
                .map_err(request_error)?;
            Ok(Subscription::from(subscription))
        })
    }

    fn subscribe_current_group<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let subscription = session
                .subscribe_current_group(&namespace, &name)
                .await
                .map_err(request_error)?;
            Ok(Subscription::from(subscription))
        })
    }

    /// PUBLISH_NAMESPACE.
    fn announce<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.announce(&namespace).await.map_err(request_error)?;
            Ok(())
        })
    }

    /// A broadcast of encoded media in `namespace`; its `announce` publishes
    /// the namespace instead of `announce`.
    fn media_broadcast(&self, namespace: Vec<String>) -> MediaBroadcast {
        // The connection spawns its tasks, which need the runtime the module's futures run on.
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        let connection = RelayConnection::with_client_session(&self.session);
        let handler = self.session.add_handler(Arc::new(connection.clone()));
        MediaBroadcast {
            broadcast: Broadcast::new(connection, namespace),
            session: self.session.clone(),
            handler,
        }
    }

    /// PUBLISH_NAMESPACE_DONE for a namespace `announce` published.
    fn unannounce<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.moqt_session().clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session
                .publisher()
                .publish_namespace_done(namespace.join("/"))
                .await?;
            Ok(())
        })
    }

    /// Registers a track with the session's origin, which keeps what is written
    /// for `retention_seconds` to answer FETCH. Its group ids start at the
    /// wall-clock time in µs, so a later run of the same publisher does not
    /// reuse a location the relay still caches.
    #[pyo3(signature = (namespace, name, *, retention_seconds))]
    fn publish_track<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        retention_seconds: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let options = PublishTrackOptions {
                retention: Retention::For(Duration::from_secs_f64(retention_seconds)),
                ..PublishTrackOptions::default()
            };
            let writer = session.publish_track(&namespace, &name, options).await?;
            Ok(TrackWriter::from(writer))
        })
    }

    /// Sends `token` to the relay as the session's new authorization token
    /// (docs/architecture/relay/architecture.md "Token refresh"), and presents
    /// it on the connections the session opens from now on, even when the
    /// relay rejects it for this connection. Fails for a session connected
    /// without a token, or when the relay rejects `token`; the relay then
    /// keeps the connection's token until it expires, and the session opens
    /// its next connection with `token`.
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
        *current.lock().unwrap() = token.clone();
        let session = self.session.moqt_session().clone();
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
            Ok(())
        })
    }

    /// Waits until the relay closes the session.
    fn closed<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.moqt_session().clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.closed().await;
            Ok(())
        })
    }

    /// Closes the session and waits until the relay has seen the close.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.close().await;
            Ok(())
        })
    }
}

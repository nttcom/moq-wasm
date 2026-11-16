use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use moqt::{
    AuthorizationToken, AuthorizationTokenSource, CertificateVerification, ClientConfig, Endpoint,
    NATIVE, Version,
    client::{
        self, FetchError, GroupStartOptions, PublishTrackOptions, RequestError, SubscribeOptions,
    },
    object::GroupOrder,
    publish::Retention,
    subscribe::FilterType,
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};

use broadcast::{Broadcast, RelayConnection};

use crate::{
    fetch::Fetch, location::Location, media_broadcast::MediaBroadcast,
    namespace_watch::NamespaceWatch, subscription::Subscription, track_writer::TrackWriter,
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

fn filter_type(filter: &str) -> PyResult<FilterType> {
    match filter {
        "largest_object" => Ok(FilterType::LargestObject),
        "next_group_start" => Ok(FilterType::NextGroupStart),
        other => Err(PyValueError::new_err(format!(
            "unknown subscribe filter: {other}"
        ))),
    }
}

fn group_start_options(
    priority: Option<u8>,
    delivery_timeout: Option<f64>,
    group_order: &str,
) -> PyResult<GroupStartOptions> {
    let defaults = GroupStartOptions::default();
    let delivery_timeout = delivery_timeout
        .map(|seconds| {
            Duration::try_from_secs_f64(seconds)
                .map_err(|_| PyValueError::new_err(format!("invalid delivery_timeout: {seconds}")))
        })
        .transpose()?;
    Ok(GroupStartOptions {
        priority: priority.unwrap_or(defaults.priority),
        delivery_timeout,
        group_order: match group_order {
            "ascending" => GroupOrder::Ascending,
            "descending" => GroupOrder::Descending,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown group order: {other}"
                )));
            }
        },
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

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        namespace, name, *, filter = "largest_object", forward = true, priority = None,
        delivery_timeout = None, group_order = "ascending"
    ))]
    fn subscribe<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        filter: &str,
        forward: bool,
        priority: Option<u8>,
        delivery_timeout: Option<f64>,
        group_order: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let delivery = group_start_options(priority, delivery_timeout, group_order)?;
        let options = SubscribeOptions {
            filter: filter_type(filter)?,
            forward,
            priority: delivery.priority,
            delivery_timeout: delivery.delivery_timeout,
            group_order: delivery.group_order,
        };
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let subscription = session
                .subscribe(&namespace, &name, options)
                .await
                .map_err(request_error)?;
            Ok(Subscription::from(subscription))
        })
    }

    #[pyo3(signature = (
        namespace, name, new_group_after, *, priority = None, delivery_timeout = None,
        group_order = "ascending"
    ))]
    #[allow(clippy::too_many_arguments)]
    fn subscribe_group_start<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        new_group_after: u64,
        priority: Option<u8>,
        delivery_timeout: Option<f64>,
        group_order: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = group_start_options(priority, delivery_timeout, group_order)?;
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let subscription = session
                .subscribe_group_start(&namespace, &name, new_group_after, options)
                .await
                .map_err(request_error)?;
            Ok(Subscription::from(subscription))
        })
    }

    /// A Standalone FETCH from `start` to `end`; `end.object_id` is the last
    /// object plus 1, and 0 means the whole group.
    fn fetch<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
        start: Location,
        end: Location,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let fetch = session
                .fetch(&namespace, &name, start.into(), end.into())
                .await
                .map_err(|error| match error {
                    FetchError::Request(error) => request_error(error),
                    error => error.into(),
                })?;
            Ok(Fetch::from(fetch))
        })
    }

    /// TRACK_STATUS: the track's largest location, or None while it has none.
    fn track_status<'py>(
        &self,
        py: Python<'py>,
        namespace: Vec<String>,
        name: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let largest = session
                .track_status(&namespace, &name)
                .await
                .map_err(request_error)?;
            Ok(largest.map(Location::from))
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

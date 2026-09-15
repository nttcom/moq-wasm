use std::sync::Arc;

use moqt::{ClientConfig, DUAL, Endpoint, ServerConfig};
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*};
use pyo3_async_runtimes::tokio::future_into_py;

use crate::session::Session;

#[pyfunction]
#[pyo3(signature = (url, *, insecure = false))]
pub(crate) fn connect<'py>(
    py: Python<'py>,
    url: String,
    insecure: bool,
) -> PyResult<Bound<'py, PyAny>> {
    future_into_py(py, async move {
        let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: !insecure,
        })?;
        let session = endpoint.connect(&url).await?.await?;
        Ok(Session::new(session))
    })
}

#[pyfunction]
#[pyo3(signature = (port, cert_path, key_path, *, keep_alive_interval_sec = 5))]
pub(crate) fn listen(
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
) -> PyResult<Server> {
    // quinn binds its socket through the ambient tokio runtime, and this
    // function is called from plain Python code outside of any future.
    let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
    let endpoint = Endpoint::<DUAL>::create_server(&ServerConfig {
        port,
        cert_path,
        key_path,
        keep_alive_interval_sec,
    })?;
    Ok(Server {
        endpoint: Arc::new(tokio::sync::Mutex::new(endpoint)),
    })
}

/// Accepts both raw QUIC (`moqt://`) and WebTransport (`https://`) clients on
/// one UDP port; the transport is negotiated per connection via ALPN.
#[pyclass(frozen)]
pub(crate) struct Server {
    endpoint: Arc<tokio::sync::Mutex<Endpoint<DUAL>>>,
}

#[pymethods]
impl Server {
    fn accept<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let endpoint = self.endpoint.clone();
        future_into_py(py, async move {
            let connecting = endpoint.lock().await.accept().await?;
            Ok(Session::new(connecting.await?))
        })
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let endpoint = self.endpoint.clone();
        future_into_py(py, async move {
            let connecting = endpoint
                .lock()
                .await
                .accept()
                .await
                .map_err(|error| PyStopAsyncIteration::new_err(error.to_string()))?;
            Ok(Session::new(connecting.await?))
        })
    }
}

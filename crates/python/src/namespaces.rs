use std::sync::Arc;

use moqt::namespace;
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*};
use tokio::sync::mpsc;

/// A namespace a peer published (`active`) or withdrew under a prefix this
/// session subscribed to.
#[pyclass(frozen, get_all, module = "moqt")]
#[derive(Clone)]
pub struct NamespaceEvent {
    pub namespace: Vec<String>,
    pub active: bool,
}

#[pyclass(module = "moqt")]
pub struct NamespaceEvents {
    receiver: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<namespace::NamespaceEvent>>>,
}

impl From<mpsc::UnboundedReceiver<namespace::NamespaceEvent>> for NamespaceEvents {
    fn from(receiver: mpsc::UnboundedReceiver<namespace::NamespaceEvent>) -> Self {
        Self {
            receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
        }
    }
}

#[pymethods]
impl NamespaceEvents {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let receiver = self.receiver.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match receiver.lock().await.recv().await {
                Some(event) => Ok(NamespaceEvent {
                    namespace: event.namespace,
                    active: event.active,
                }),
                None => Err(PyStopAsyncIteration::new_err("the session closed")),
            }
        })
    }
}

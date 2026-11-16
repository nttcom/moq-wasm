use std::sync::Arc;

use moqt::client;
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*};

/// A namespace a peer published (`active`) or withdrew under a prefix this
/// session subscribed to.
#[pyclass(frozen, get_all, module = "moq_wasm")]
#[derive(Clone)]
pub struct NamespaceEvent {
    pub namespace: Vec<String>,
    pub active: bool,
}

#[pyclass(module = "moq_wasm")]
pub struct NamespaceWatch {
    watch: Arc<client::NamespaceWatch>,
}

impl From<client::NamespaceWatch> for NamespaceWatch {
    fn from(watch: client::NamespaceWatch) -> Self {
        Self {
            watch: Arc::new(watch),
        }
    }
}

#[pymethods]
impl NamespaceWatch {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let watch = self.watch.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match watch.next().await {
                Some(event) => Ok(NamespaceEvent {
                    namespace: event.namespace,
                    active: event.active,
                }),
                None => Err(PyStopAsyncIteration::new_err("the session closed")),
            }
        })
    }
}

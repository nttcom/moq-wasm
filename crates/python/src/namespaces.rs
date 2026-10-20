use std::sync::{Arc, Mutex};

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

/// Every `subscribe_namespace` iterator of a session, fed by its event loop.
#[derive(Default)]
pub(crate) struct NamespaceWatchers {
    watchers: Mutex<Vec<(Vec<String>, mpsc::UnboundedSender<NamespaceEvent>)>>,
}

impl NamespaceWatchers {
    pub(crate) fn watch(&self, prefix: Vec<String>) -> NamespaceEvents {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.watchers.lock().unwrap().push((prefix, sender));
        NamespaceEvents {
            receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
        }
    }

    pub(crate) fn notify(&self, namespace: Vec<String>, active: bool) {
        let event = NamespaceEvent { namespace, active };
        self.watchers.lock().unwrap().retain(|(prefix, sender)| {
            !event.namespace.starts_with(prefix) || sender.send(event.clone()).is_ok()
        });
    }

    /// Ends every iterator, once the session is gone.
    pub(crate) fn close(&self) {
        self.watchers.lock().unwrap().clear();
    }
}

#[pyclass(module = "moqt")]
pub struct NamespaceEvents {
    receiver: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<NamespaceEvent>>>,
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
                Some(event) => Ok(event),
                None => Err(PyStopAsyncIteration::new_err("the session closed")),
            }
        })
    }
}

use std::sync::Arc;

use crate::scope::SessionScope;
use moqt::client;
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*};

use crate::subscription::Object;

/// The objects of one FETCH response that carry a payload, in order.
#[pyclass(module = "moq_wasm")]
pub struct Fetch {
    fetch: Arc<client::Fetch>,
    scope: SessionScope,
}

impl Fetch {
    pub(crate) fn new(fetch: client::Fetch, scope: SessionScope) -> Self {
        Self {
            fetch: Arc::new(fetch),
            scope,
        }
    }
}

#[pymethods]
impl Fetch {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let fetch = self.fetch.clone();
        self.scope.future_into_py(py, async move {
            match fetch.next().await? {
                Some(object) => Ok(Object::from(object)),
                None => Err(PyStopAsyncIteration::new_err("the response ended")),
            }
        })
    }
}

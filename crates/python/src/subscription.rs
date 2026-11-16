use std::sync::Arc;

use bytes::Bytes;
use moqt::client::{self, ReceivedObject};
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*, types::PyBytes};

#[pyclass(frozen, module = "moq_wasm")]
pub struct Object {
    #[pyo3(get)]
    group_id: u64,
    #[pyo3(get)]
    object_id: u64,
    payload: Bytes,
}

#[pymethods]
impl Object {
    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload)
    }
}

impl From<ReceivedObject> for Object {
    fn from(object: ReceivedObject) -> Self {
        Self {
            group_id: object.group_id,
            object_id: object.object_id,
            payload: object.payload,
        }
    }
}

/// A subscription, iterated for its objects until the publisher ends it.
#[pyclass(module = "moq_wasm")]
pub struct Subscription {
    subscription: Arc<client::Subscription>,
}

impl From<client::Subscription> for Subscription {
    fn from(subscription: client::Subscription) -> Self {
        Self {
            subscription: Arc::new(subscription),
        }
    }
}

#[pymethods]
impl Subscription {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match subscription.next().await? {
                Some(object) => Ok(Object::from(object)),
                None => Err(PyStopAsyncIteration::new_err("the track ended")),
            }
        })
    }

    /// Sends UNSUBSCRIBE; the iterator ends once the relay stops the track.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            subscription.close().await?;
            Ok(())
        })
    }
}

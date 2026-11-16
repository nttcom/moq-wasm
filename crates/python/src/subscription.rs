use std::sync::Arc;

use crate::scope::SessionScope;
use bytes::Bytes;
use moqt::{
    client::{self, ReceivedObject},
    wire::publish_done_code_to_wire,
};
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*, types::PyBytes};

use crate::location::Location;

#[pyclass(frozen, module = "moq_wasm")]
pub struct Object {
    #[pyo3(get)]
    group_id: u64,
    #[pyo3(get)]
    subgroup_id: u64,
    #[pyo3(get)]
    object_id: u64,
    #[pyo3(get)]
    status: Option<u8>,
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
            subgroup_id: object.subgroup_id,
            object_id: object.object_id,
            status: object.status.map(|status| status as u8),
            payload: object.payload,
        }
    }
}

/// The PUBLISH_DONE that ended a subscription.
#[pyclass(frozen, get_all, module = "moq_wasm")]
pub struct PublishDone {
    /// The PUBLISH_DONE code as draft-14 numbers it (§13.1.3), also on a
    /// draft-18 session.
    pub code: u64,
    pub reason: String,
}

/// A subscription, iterated for its objects until the publisher ends it.
#[pyclass(module = "moq_wasm")]
pub struct Subscription {
    subscription: Arc<client::Subscription>,
    scope: SessionScope,
}

impl Subscription {
    pub(crate) fn new(subscription: client::Subscription, scope: SessionScope) -> Self {
        Self {
            subscription: Arc::new(subscription),
            scope,
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
        self.scope.future_into_py(py, async move {
            match subscription.next().await? {
                Some(object) => Ok(Object::from(object)),
                None => Err(PyStopAsyncIteration::new_err("the track ended")),
            }
        })
    }

    #[getter]
    fn largest(&self) -> Option<Location> {
        self.subscription.largest().map(Location::from)
    }

    fn done<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let done = self.subscription.done();
        self.scope.future_into_py(py, async move {
            Ok(done.await.map(|done| PublishDone {
                code: publish_done_code_to_wire(done.code),
                reason: done.reason,
            }))
        })
    }

    fn groups(&self) -> Groups {
        Groups {
            subscription: self.subscription.clone(),
            scope: self.scope,
        }
    }

    fn set_forward<'py>(&self, py: Python<'py>, forward: bool) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        self.scope.future_into_py(py, async move {
            subscription.set_forward(forward).await?;
            Ok(())
        })
    }

    fn set_priority<'py>(&self, py: Python<'py>, priority: u8) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        self.scope.future_into_py(py, async move {
            subscription.set_priority(priority).await?;
            Ok(())
        })
    }

    #[pyo3(signature = (largest_group = None))]
    fn request_new_group<'py>(
        &self,
        py: Python<'py>,
        largest_group: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        self.scope.future_into_py(py, async move {
            Ok(subscription.request_new_group(largest_group).await?)
        })
    }

    fn end_before_group<'py>(&self, py: Python<'py>, group_id: u64) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        self.scope.future_into_py(py, async move {
            subscription.end_before_group(group_id).await?;
            Ok(())
        })
    }

    /// Sends UNSUBSCRIBE; the iterator ends once the relay stops the track.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        self.scope.future_into_py(py, async move {
            subscription.close().await?;
            Ok(())
        })
    }
}

/// The subgroup streams of a subscription.
#[pyclass(module = "moq_wasm")]
pub struct Groups {
    subscription: Arc<client::Subscription>,
    scope: SessionScope,
}

#[pymethods]
impl Groups {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let subscription = self.subscription.clone();
        let scope = self.scope;
        self.scope.future_into_py(py, async move {
            match subscription.next_group().await? {
                Some(group) => Ok(Group {
                    group_id: group.group_id(),
                    subgroup_id: group.subgroup_id(),
                    group: Arc::new(group),
                    scope,
                }),
                None => Err(PyStopAsyncIteration::new_err("the track ended")),
            }
        })
    }
}

/// One subgroup stream, iterated for its objects until the publisher
/// finishes it.
#[pyclass(module = "moq_wasm")]
pub struct Group {
    #[pyo3(get)]
    group_id: u64,
    #[pyo3(get)]
    subgroup_id: u64,
    group: Arc<client::Group>,
    scope: SessionScope,
}

#[pymethods]
impl Group {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let group = self.group.clone();
        self.scope.future_into_py(py, async move {
            match group.next().await? {
                Some(object) => Ok(Object::from(object)),
                None => Err(PyStopAsyncIteration::new_err("the stream ended")),
            }
        })
    }
}

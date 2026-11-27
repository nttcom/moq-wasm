use std::sync::Arc;

use crate::scope::SessionScope;
use mediapack::loc::{LocObject, LocValue, from_extension_headers};
use moqt::client::{self, ReceivedObject};
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
    /// The payload and the extension headers, which carry the object's LOC
    /// properties (draft-ietf-moq-loc-04 §2.2).
    loc: LocObject,
}

#[pymethods]
impl Object {
    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.loc.payload)
    }

    /// The extension headers as `(id, value)` pairs in the order they came:
    /// an even id carries an integer, an odd one bytes.
    #[getter]
    fn extension_headers<'py>(&self, py: Python<'py>) -> PyResult<Vec<(u64, Bound<'py, PyAny>)>> {
        self.loc
            .extensions
            .iter()
            .map(|extension| {
                let value = match &extension.value {
                    LocValue::Varint(value) => value.into_pyobject(py)?.into_any(),
                    LocValue::Bytes(bytes) => PyBytes::new(py, bytes).into_any(),
                };
                Ok((extension.id, value))
            })
            .collect()
    }

    /// The LOC Timestamp property in µs (draft-ietf-moq-loc-04 §2.3.1):
    /// wall-clock time since the Unix epoch, or media time when a Timescale
    /// property comes with it; None without one.
    #[getter]
    fn timestamp_micros(&self) -> Option<u64> {
        self.loc.timestamp().map(|timestamp| timestamp.micros())
    }
}

impl From<ReceivedObject> for Object {
    fn from(object: ReceivedObject) -> Self {
        Self {
            group_id: object.group_id,
            subgroup_id: object.subgroup_id,
            object_id: object.object_id,
            status: object.status.map(|status| status as u8),
            loc: LocObject {
                extensions: from_extension_headers(&object.extension_headers),
                payload: object.payload,
            },
        }
    }
}

/// The PUBLISH_DONE that ended a subscription.
#[pyclass(frozen, get_all, module = "moq_wasm")]
pub struct PublishDone {
    /// The PUBLISH_DONE code's name, e.g. `TRACK_ENDED`, the same on every
    /// draft.
    pub code: String,
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
                code: done.code.name().to_string(),
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
            match group.next().await.map_err(anyhow::Error::from)? {
                Some(object) => Ok(Object::from(object)),
                None => Err(PyStopAsyncIteration::new_err("the stream ended")),
            }
        })
    }
}

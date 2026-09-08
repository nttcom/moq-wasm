use std::sync::Arc;

use bytes::Bytes;
use moqt::{DUAL, DataReceiver, Subscription, TrackObject};
use pyo3::{
    exceptions::PyStopAsyncIteration,
    prelude::*,
    types::{PyBytes, PyList},
};
use pyo3_async_runtimes::tokio::future_into_py;

use crate::session::SharedSession;

/// A data receiver only exists once the first object of the track has
/// arrived, so the moqt reader is created lazily on the first read instead
/// of blocking `subscribe()` / `accept()` until the publisher sends.
enum ReaderState {
    Pending {
        session: SharedSession,
        subscription: Subscription,
    },
    Ready(moqt::TrackReader<DUAL>),
}

impl ReaderState {
    async fn reader(&mut self) -> anyhow::Result<&mut moqt::TrackReader<DUAL>> {
        if let Self::Pending {
            session,
            subscription,
        } = self
        {
            let receiver = session
                .subscriber()
                .accept_data_receiver(subscription)
                .await?;
            let DataReceiver::Stream(factory) = receiver else {
                anyhow::bail!("datagram tracks are not supported");
            };
            *self = Self::Ready(moqt::TrackReader::new(factory));
        }
        match self {
            Self::Ready(reader) => Ok(reader),
            Self::Pending { .. } => unreachable!("state was set to Ready above"),
        }
    }
}

#[pyclass(frozen)]
pub(crate) struct TrackReader {
    state: Arc<tokio::sync::Mutex<ReaderState>>,
}

impl TrackReader {
    pub(crate) fn pending(session: SharedSession, subscription: Subscription) -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(ReaderState::Pending {
                session,
                subscription,
            })),
        }
    }
}

#[pymethods]
impl TrackReader {
    /// Resolves to the next `MoqObject`, or `None` when the track ends.
    fn next_object<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let state = self.state.clone();
        future_into_py(py, async move {
            let object = state.lock().await.reader().await?.next_object().await?;
            Ok(object.map(MoqObject::from))
        })
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let state = self.state.clone();
        future_into_py(py, async move {
            match state.lock().await.reader().await?.next_object().await? {
                Some(object) => Ok(MoqObject::from(object)),
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }
}

#[pyclass(frozen)]
pub(crate) struct MoqObject {
    #[pyo3(get)]
    group_id: u64,
    #[pyo3(get)]
    object_id: u64,
    immutable_extensions: Vec<Bytes>,
    payload: Bytes,
}

impl From<TrackObject> for MoqObject {
    fn from(object: TrackObject) -> Self {
        Self {
            group_id: object.group_id,
            object_id: object.object_id,
            immutable_extensions: object.extension_headers.immutable_extensions(),
            payload: object.payload,
        }
    }
}

#[pymethods]
impl MoqObject {
    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload)
    }

    #[getter]
    fn immutable_extensions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(
            py,
            self.immutable_extensions
                .iter()
                .map(|extension| PyBytes::new(py, extension)),
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "MoqObject(group_id={}, object_id={}, payload_len={}, immutable_extensions={})",
            self.group_id,
            self.object_id,
            self.payload.len(),
            self.immutable_extensions.len()
        )
    }
}

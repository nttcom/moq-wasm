use std::sync::Arc;

use crate::scope::SessionScope;
use bytes::Bytes;
use moqt::{client, object::ExtensionHeaders};
use pyo3::prelude::*;

/// A track this session publishes through its origin, which numbers and caches
/// what is written and answers the track's SUBSCRIBEs and FETCHes.
#[pyclass(module = "moq_wasm")]
pub struct TrackWriter {
    writer: Arc<client::TrackWriter>,
    scope: SessionScope,
}

impl TrackWriter {
    pub(crate) fn new(writer: client::TrackWriter, scope: SessionScope) -> Self {
        Self {
            writer: Arc::new(writer),
            scope,
        }
    }
}

#[pymethods]
impl TrackWriter {
    #[pyo3(signature = (payload, *, new_group = false))]
    fn write<'py>(
        &self,
        py: Python<'py>,
        payload: Vec<u8>,
        new_group: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let writer = self.writer.clone();
        self.scope.future_into_py(py, async move {
            Ok(writer
                .write(Bytes::from(payload), new_group, ExtensionHeaders::default())
                .await?)
        })
    }

    fn write_group<'py>(&self, py: Python<'py>, payload: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
        let writer = self.writer.clone();
        self.scope.future_into_py(py, async move {
            Ok(writer.write_group(Bytes::from(payload)).await?)
        })
    }

    fn next_subscriber<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let writer = self.writer.clone();
        pyo3_async_runtimes::tokio::future_into_py(
            py,
            async move { Ok(writer.next_subscriber().await) },
        )
    }
}

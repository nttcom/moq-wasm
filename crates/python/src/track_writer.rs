use std::sync::Arc;

use bytes::Bytes;
use moqt::{client, object::ExtensionHeaders};
use pyo3::prelude::*;

/// A track this session publishes through its origin, which numbers and caches
/// what is written and answers the track's SUBSCRIBEs and FETCHes.
#[pyclass(module = "moq_wasm")]
pub struct TrackWriter {
    writer: Arc<client::TrackWriter>,
}

impl From<client::TrackWriter> for TrackWriter {
    fn from(writer: client::TrackWriter) -> Self {
        Self {
            writer: Arc::new(writer),
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
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            writer
                .write(Bytes::from(payload), new_group, ExtensionHeaders::default())
                .await?;
            Ok(())
        })
    }
}

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use moqt::DUAL;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

/// Publishers must not reuse group ids across runs (a relay treats a
/// republished group 0 as a duplicate location), so the default first group
/// id is the wall clock in microseconds.
pub(crate) fn first_group_id_or_now(first_group_id: Option<u64>) -> u64 {
    first_group_id.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as u64)
            .unwrap_or(0)
    })
}

#[pyclass(frozen)]
pub(crate) struct TrackWriter {
    inner: Arc<tokio::sync::Mutex<Option<moqt::TrackWriter<DUAL>>>>,
}

impl TrackWriter {
    pub(crate) fn new(writer: moqt::TrackWriter<DUAL>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(Some(writer))),
        }
    }
}

fn finished_error() -> anyhow::Error {
    anyhow::anyhow!("this TrackWriter has already been finished")
}

#[pymethods]
impl TrackWriter {
    fn start_group<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner
                .lock()
                .await
                .as_mut()
                .ok_or_else(finished_error)?
                .start_group()
                .await?;
            Ok(())
        })
    }

    #[pyo3(signature = (payload, *, immutable_extensions = Vec::new()))]
    fn write<'py>(
        &self,
        py: Python<'py>,
        payload: Vec<u8>,
        immutable_extensions: Vec<Vec<u8>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let extensions = immutable_extensions.into_iter().map(Bytes::from).collect();
            inner
                .lock()
                .await
                .as_mut()
                .ok_or_else(finished_error)?
                .write(Bytes::from(payload), extensions)
                .await?;
            Ok(())
        })
    }

    fn write_group<'py>(&self, py: Python<'py>, payload: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner
                .lock()
                .await
                .as_mut()
                .ok_or_else(finished_error)?
                .write_group(Bytes::from(payload))
                .await?;
            Ok(())
        })
    }

    fn finish<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner
                .lock()
                .await
                .take()
                .ok_or_else(finished_error)?
                .finish()
                .await?;
            Ok(())
        })
    }

    fn groups<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            Ok(inner
                .lock()
                .await
                .as_ref()
                .ok_or_else(finished_error)?
                .groups())
        })
    }
}

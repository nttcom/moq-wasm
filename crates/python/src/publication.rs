use bytes::Bytes;
use moqt::publish::{GroupBoundary, Origin, OutgoingObject};
use pyo3::prelude::*;

/// A track this session publishes through its origin, which numbers and caches
/// what is written and answers the track's SUBSCRIBEs and FETCHes.
#[pyclass(module = "moqt")]
pub struct TrackPublication {
    pub(crate) origin: Origin,
    pub(crate) namespace: String,
    pub(crate) name: String,
}

#[pymethods]
impl TrackPublication {
    #[pyo3(signature = (payload, *, new_group = false))]
    fn write<'py>(
        &self,
        py: Python<'py>,
        payload: Vec<u8>,
        new_group: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let (origin, namespace, name) = (
            self.origin.clone(),
            self.namespace.clone(),
            self.name.clone(),
        );
        let group = if new_group {
            GroupBoundary::Next
        } else {
            GroupBoundary::Within
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            origin
                .write(
                    &namespace,
                    &name,
                    OutgoingObject::plain(group, Bytes::from(payload)),
                )
                .await?;
            Ok(())
        })
    }
}

use msf::Catalog;
use pyo3::{exceptions::PyValueError, prelude::*, types::PyBytes};

#[pyfunction]
pub(crate) fn parse_catalog<'py>(py: Python<'py>, payload: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    Catalog::parse(payload).map_err(|error| PyValueError::new_err(error.to_string()))?;
    py.import("json")?
        .call_method1("loads", (PyBytes::new(py, payload),))
}

use std::future::Future;

use pyo3::{conversion::IntoPyObject, prelude::*};
use telemetry_report::{in_scope, scoped};

/// The `telemetry_report` scope of one native session, which the log sink
/// maps to the session's token: what the session and the objects it returns
/// record carries it.
#[derive(Clone, Copy)]
pub(crate) struct SessionScope(pub(crate) u64);

impl SessionScope {
    /// `pyo3_async_runtimes::tokio::future_into_py`, polling `future` in the
    /// scope.
    pub(crate) fn future_into_py<'py, F, T>(
        self,
        py: Python<'py>,
        future: F,
    ) -> PyResult<Bound<'py, PyAny>>
    where
        F: Future<Output = PyResult<T>> + Send + 'static,
        T: for<'a> IntoPyObject<'a> + Send + 'static,
    {
        pyo3_async_runtimes::tokio::future_into_py(py, scoped(self.0, future))
    }

    /// Runs `f` in the scope.
    pub(crate) fn run<R>(self, f: impl FnOnce() -> R) -> R {
        in_scope(self.0, f)
    }
}

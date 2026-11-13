mod catalog;
mod namespace_watch;
mod session;
mod subscription;
mod track_writer;

use pyo3::prelude::*;

/// The native module of the `moq-wasm` Python SDK: bindings to this repository's
/// MoQT implementation (see docs/architecture/python).
#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(session::connect, module)?)?;
    module.add_function(wrap_pyfunction!(catalog::parse_catalog, module)?)?;
    module.add_class::<session::Session>()?;
    module.add_class::<namespace_watch::NamespaceEvent>()?;
    module.add_class::<namespace_watch::NamespaceWatch>()?;
    module.add_class::<subscription::Subscription>()?;
    module.add_class::<subscription::Object>()?;
    module.add_class::<track_writer::TrackWriter>()?;
    Ok(())
}

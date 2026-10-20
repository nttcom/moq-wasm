mod namespaces;
mod publication;
mod session;
mod track;

use pyo3::prelude::*;

/// Python bindings for this repository's MoQT implementation, covering what a bot
/// that follows a room and publishes into it needs (see docs/architecture/python).
#[pymodule]
fn moqt(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(session::connect, module)?)?;
    module.add_class::<session::Session>()?;
    module.add_class::<namespaces::NamespaceEvent>()?;
    module.add_class::<namespaces::NamespaceEvents>()?;
    module.add_class::<track::Track>()?;
    module.add_class::<track::Object>()?;
    module.add_class::<publication::TrackPublication>()?;
    Ok(())
}

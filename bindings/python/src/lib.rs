mod endpoint;
mod events;
mod session;
mod track_reader;
mod track_writer;

use pyo3::prelude::*;

#[pymodule]
fn moqt(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(endpoint::connect, module)?)?;
    module.add_function(wrap_pyfunction!(endpoint::listen, module)?)?;
    module.add_class::<endpoint::Server>()?;
    module.add_class::<session::Session>()?;
    module.add_class::<events::SubscribeRequest>()?;
    module.add_class::<events::PublishRequest>()?;
    module.add_class::<events::PublishNamespaceRequest>()?;
    module.add_class::<events::SubscribeNamespaceRequest>()?;
    module.add_class::<events::Disconnected>()?;
    module.add_class::<events::ProtocolViolation>()?;
    module.add_class::<track_reader::TrackReader>()?;
    module.add_class::<track_reader::MoqObject>()?;
    module.add_class::<track_writer::TrackWriter>()?;
    Ok(())
}

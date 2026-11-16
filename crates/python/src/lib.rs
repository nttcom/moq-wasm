mod catalog;
mod errors;
mod fetch;
mod location;
mod media_broadcast;
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
    module.add_function(wrap_pyfunction!(
        media_broadcast::check_audio_specific_config,
        module
    )?)?;
    module.add_class::<session::Session>()?;
    module.add_class::<errors::CloseInfo>()?;
    module.add_class::<errors::TransportStats>()?;
    module.add(
        "RequestError",
        module.py().get_type::<errors::RequestError>(),
    )?;
    module.add_class::<namespace_watch::NamespaceEvent>()?;
    module.add_class::<namespace_watch::NamespaceWatch>()?;
    module.add_class::<subscription::Subscription>()?;
    module.add_class::<subscription::Object>()?;
    module.add_class::<subscription::PublishDone>()?;
    module.add_class::<subscription::Groups>()?;
    module.add_class::<subscription::Group>()?;
    module.add_class::<fetch::Fetch>()?;
    module.add_class::<location::Location>()?;
    module.add_class::<track_writer::TrackWriter>()?;
    module.add_class::<media_broadcast::MediaBroadcast>()?;
    module.add_class::<media_broadcast::MediaSource>()?;
    module.add_class::<media_broadcast::Rendition>()?;
    module.add_class::<media_broadcast::Demand>()?;
    Ok(())
}

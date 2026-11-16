pub(crate) mod announcement;
pub(crate) mod event_handler;
mod event_loop_task;
pub(crate) mod fetch;
pub(crate) mod namespace_watch;
pub(crate) mod request_error;
mod routes;
pub(crate) mod session;
pub(crate) mod subscription;
pub(crate) mod track_writer;

/// The `/`-joined form `moqt::Session` takes.
fn join_namespace(namespace: &[impl AsRef<str>]) -> String {
    namespace
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("/")
}

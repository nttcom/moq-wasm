pub(crate) mod announcement;
pub(crate) mod event_handler;
mod event_loop_task;
pub(crate) mod fetch;
pub(crate) mod namespace_watch;
pub(crate) mod request_error;
mod routes;
pub(crate) mod session;
pub(crate) mod subscription;
pub(crate) mod track_follower;
pub(crate) mod track_watch;
pub(crate) mod track_writer;

use crate::RequestFailure;

/// The `/`-joined form `moqt::Session` takes, which splits it back at `/`
/// into the Track Namespace tuple.
fn join_namespace(namespace: &[impl AsRef<str>]) -> Result<String, RequestFailure> {
    let elements: Vec<&str> = namespace.iter().map(AsRef::as_ref).collect();
    if elements.iter().any(|element| element.contains('/')) {
        return Err(RequestFailure::InvalidNamespace);
    }
    Ok(elements.join("/"))
}

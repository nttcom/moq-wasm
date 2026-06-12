use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace_done::PublishNamespaceDone;

#[derive(Clone, Debug)]
pub struct PublishNamespaceDoneHandler {
    pub track_namespace: String,
}

impl PublishNamespaceDoneHandler {
    pub(crate) fn new(publish_namespace_done: PublishNamespaceDone) -> Self {
        Self {
            track_namespace: publish_namespace_done.track_namespace.join("/"),
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

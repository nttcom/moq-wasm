use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace_done::PublishNamespaceDone;

#[derive(Clone, Debug)]
pub struct PublishNamespaceDoneHandler {
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
}

impl PublishNamespaceDoneHandler {
    pub(crate) fn new(publish_namespace_done: PublishNamespaceDone) -> Self {
        Self {
            track_namespace: publish_namespace_done.track_namespace.join("/"),
            track_namespace_tuple: publish_namespace_done.track_namespace,
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }
}

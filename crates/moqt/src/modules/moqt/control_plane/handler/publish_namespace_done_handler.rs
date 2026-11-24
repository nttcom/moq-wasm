use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace_done::PublishNamespaceDone;

#[derive(Clone, Debug)]
pub struct PublishNamespaceDoneHandler {
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
    pub request_id: Option<u64>,
}

impl PublishNamespaceDoneHandler {
    pub(crate) fn new(
        publish_namespace_done: PublishNamespaceDone,
        request_id: Option<u64>,
    ) -> Self {
        Self {
            track_namespace: publish_namespace_done.track_namespace.join("/"),
            track_namespace_tuple: publish_namespace_done.track_namespace,
            request_id,
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    pub fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    /// The Request ID of the PUBLISH_NAMESPACE this withdraws, as its
    /// `PublishNamespaceHandler::request_id` gave it. `None` on draft-14,
    /// whose PUBLISH_NAMESPACE_DONE carries only the namespace (§9.26).
    pub fn request_id(&self) -> Option<u64> {
        self.request_id
    }
}

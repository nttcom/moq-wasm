use crate::modules::moqt::control_plane::control_messages::messages::unsubscribe_namespace::UnsubscribeNamespace;

#[derive(Clone, Debug)]
pub struct UnsubscribeNamespaceHandler {
    pub track_namespace_prefix: String,
}

impl UnsubscribeNamespaceHandler {
    pub(crate) fn new(unsubscribe_namespace: UnsubscribeNamespace) -> Self {
        Self {
            track_namespace_prefix: unsubscribe_namespace.track_namespace_prefix.join("/"),
        }
    }

    pub fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }
}

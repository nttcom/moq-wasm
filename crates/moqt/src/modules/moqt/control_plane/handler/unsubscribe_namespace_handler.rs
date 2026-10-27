use crate::modules::moqt::control_plane::{
    control_messages::messages::unsubscribe_namespace::UnsubscribeNamespace,
    namespace_interest::NamespaceInterest,
};

#[derive(Clone, Debug)]
pub struct UnsubscribeNamespaceHandler {
    pub track_namespace_prefix: String,
    /// What the withdrawn subscription asked for: one of a draft-18
    /// session's SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS for the same prefix
    /// can end without the other.
    pub interest: NamespaceInterest,
}

impl UnsubscribeNamespaceHandler {
    pub(crate) fn new(
        unsubscribe_namespace: UnsubscribeNamespace,
        interest: NamespaceInterest,
    ) -> Self {
        Self {
            track_namespace_prefix: unsubscribe_namespace.track_namespace_prefix.join("/"),
            interest,
        }
    }

    pub fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }
}

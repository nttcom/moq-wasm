type PublisherId = usize;
type SubscriberId = usize;

pub(crate) enum MOQTEvent {
    NamespacePublished(SubscriberId, String),
    NamespaceSubscribed(PublisherId, String),
    Publish(),
    Subscribe(),
}

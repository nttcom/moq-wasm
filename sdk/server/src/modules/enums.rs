pub(crate) enum MOQTEvent {
    PublishNamespace(String),
    SubscribeNameSpace(String),
    Publish(),
    Subscribe()
}
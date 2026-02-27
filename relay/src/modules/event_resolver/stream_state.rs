pub(crate) enum StreamCommand {
    SubscribeUpdate,
    Unsubscribe,
    PublishDone,
}

pub(crate) struct StreamState {
    pub(crate) state: StreamCommand,
}

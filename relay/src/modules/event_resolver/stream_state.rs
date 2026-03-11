// This file defines the state of the stream in the event resolver module.
// It is used to track the current state of the stream and determine the appropriate actions to take based on the state.
pub(crate) enum _StreamCommand {
    SubscribeUpdate,
    Unsubscribe,
    PublishDone,
}

pub(crate) struct _StreamState {
    pub(crate) state: _StreamCommand,
}

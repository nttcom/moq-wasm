use thiserror::Error;

/// Draft-14 §2.5: the subscriber MUST UNSUBSCRIBE a malformed track; the
/// receiver only reports it, the caller sends the UNSUBSCRIBE.
#[derive(Debug, Error)]
#[error("malformed track: object received with a different forwarding preference")]
pub struct MalformedTrackError;

use std::fmt;

/// Draft-14 §2.5: the subscriber MUST UNSUBSCRIBE a malformed track; the
/// receiver only reports it, the caller sends the UNSUBSCRIBE.
#[derive(Debug)]
pub struct MalformedTrackError;

impl fmt::Display for MalformedTrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "malformed track: object received with a different forwarding preference"
        )
    }
}

impl std::error::Error for MalformedTrackError {}

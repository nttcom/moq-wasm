use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub(crate) enum MOQTMessageError {
    #[error("Protocol violation.")]
    ProtocolViolation,
    #[error("Message unmatches")]
    MessageUnmatches
}
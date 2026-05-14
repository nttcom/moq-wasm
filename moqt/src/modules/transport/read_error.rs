use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The stream has been closed.
    #[error("stream closed by peer")]
    Closed,
    /// The stream has been reset by the peer.
    #[error("stream reset by peer")]
    Reset,
    /// An error occurred while reading from the stream.
    #[error("connection lost")]
    ConnectionLost,
}

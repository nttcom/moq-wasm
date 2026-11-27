use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The stream has been closed.
    #[error("stream closed by peer")]
    Closed,
    /// The peer reset the stream with this application error code.
    #[error("stream reset by peer with code {0}")]
    Reset(u64),
    /// An error occurred while reading from the stream.
    #[error("connection lost")]
    ConnectionLost,
}

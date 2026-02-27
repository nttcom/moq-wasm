use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Need more data to decode.
    #[error("need more data to decode")]
    NeedMoreData,
    #[error("fatal error: {0}")]
    Fatal(String),
}

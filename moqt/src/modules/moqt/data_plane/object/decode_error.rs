use bytes::TryGetError;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("need more data to decode")]
    NeedMoreData,
    #[error("fatal error: {0}")]
    Fatal(String),
}

impl From<TryGetError> for DecodeError {
    fn from(_: TryGetError) -> Self {
        Self::NeedMoreData
    }
}

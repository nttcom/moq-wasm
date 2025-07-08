use bytes::BytesMut;

use crate::modules::session_handlers::constants::TerminationErrorCode;

#[derive(Debug, PartialEq)]
pub enum MessageProcessResult {
    Success(BytesMut),
    SuccessWithoutResponse,
    Failure(TerminationErrorCode, String),
    Fragment,
}

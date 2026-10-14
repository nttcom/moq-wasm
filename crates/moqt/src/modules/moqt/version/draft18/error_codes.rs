//! The values draft-18 gives the error codes (§15.10).

use crate::modules::moqt::control_plane::{
    constants::TerminationErrorCode,
    error_codes::{DataStreamResetCode, PublishDoneCode, RequestErrorCode},
};

/// §15.10.2. draft-18 numbers request errors once for every request kind.
const REQUEST_ERROR: [(RequestErrorCode, u64); 18] = [
    (RequestErrorCode::InternalError, 0x0),
    (RequestErrorCode::Unauthorized, 0x1),
    (RequestErrorCode::Timeout, 0x2),
    (RequestErrorCode::NotSupported, 0x3),
    (RequestErrorCode::MalformedAuthToken, 0x4),
    (RequestErrorCode::ExpiredAuthToken, 0x5),
    (RequestErrorCode::GoingAway, 0x6),
    (RequestErrorCode::ExcessiveLoad, 0x9),
    (RequestErrorCode::TrackDoesNotExist, 0x10),
    (RequestErrorCode::InvalidRange, 0x11),
    (RequestErrorCode::MalformedTrack, 0x12),
    (RequestErrorCode::DuplicateSubscription, 0x19),
    (RequestErrorCode::Uninterested, 0x20),
    (RequestErrorCode::NamespacePrefixOverlap, 0x30),
    (RequestErrorCode::NamespaceTooLarge, 0x31),
    (RequestErrorCode::InvalidJoiningRequestId, 0x32),
    (RequestErrorCode::UnsupportedExtension, 0x33),
    (RequestErrorCode::Redirect, 0x34),
];

pub(crate) fn request_error_code_to_wire(code: RequestErrorCode) -> u64 {
    let code = match code {
        // draft-18 folds these draft-14 codes into broader ones.
        RequestErrorCode::NamespacePrefixUnknown => RequestErrorCode::TrackDoesNotExist,
        RequestErrorCode::NoObjects => RequestErrorCode::InvalidRange,
        RequestErrorCode::UnknownStatusInRange => RequestErrorCode::InternalError,
        code => code,
    };
    REQUEST_ERROR
        .iter()
        .find(|(defined, _)| *defined == code)
        .map(|(_, value)| *value)
        .unwrap_or_else(|| {
            tracing::warn!(
                ?code,
                "request error code undefined in draft-18; sending INTERNAL_ERROR"
            );
            0x0
        })
}

/// §10.6.2: an unknown code reads as INTERNAL_ERROR.
pub(crate) fn request_error_code_from_wire(value: u64) -> RequestErrorCode {
    REQUEST_ERROR
        .iter()
        .find(|(_, defined)| *defined == value)
        .map(|(code, _)| *code)
        .unwrap_or(RequestErrorCode::InternalError)
}

/// §15.10.3.
const PUBLISH_DONE: [(PublishDoneCode, u64); 10] = [
    (PublishDoneCode::InternalError, 0x0),
    (PublishDoneCode::Unauthorized, 0x1),
    (PublishDoneCode::TrackEnded, 0x2),
    (PublishDoneCode::SubscriptionEnded, 0x3),
    (PublishDoneCode::GoingAway, 0x4),
    (PublishDoneCode::TooFarBehind, 0x5),
    (PublishDoneCode::Expired, 0x6),
    (PublishDoneCode::UpdateFailed, 0x8),
    (PublishDoneCode::ExcessiveLoad, 0x9),
    (PublishDoneCode::MalformedTrack, 0x12),
];

pub(crate) fn publish_done_code_to_wire(code: PublishDoneCode) -> u64 {
    PUBLISH_DONE
        .iter()
        .find(|(defined, _)| *defined == code)
        .map(|(_, value)| *value)
        .unwrap_or_else(|| {
            tracing::warn!(
                ?code,
                "PUBLISH_DONE code undefined in draft-18; sending INTERNAL_ERROR"
            );
            0x0
        })
}

pub(crate) fn publish_done_code_from_wire(value: u64) -> PublishDoneCode {
    PUBLISH_DONE
        .iter()
        .find(|(_, defined)| *defined == value)
        .map(|(code, _)| *code)
        .unwrap_or(PublishDoneCode::InternalError)
}

/// §15.10.4.
const STREAM_RESET: [(DataStreamResetCode, u64); 10] = [
    (DataStreamResetCode::InternalError, 0x0),
    (DataStreamResetCode::Cancelled, 0x1),
    (DataStreamResetCode::DeliveryTimeout, 0x2),
    (DataStreamResetCode::SessionClosed, 0x3),
    (DataStreamResetCode::GoingAway, 0x4),
    (DataStreamResetCode::TooFarBehind, 0x5),
    (DataStreamResetCode::UnknownObjectStatus, 0x6),
    (DataStreamResetCode::ExpiredAuthToken, 0x7),
    (DataStreamResetCode::ExcessiveLoad, 0x9),
    (DataStreamResetCode::MalformedTrack, 0x12),
];

pub(crate) fn stream_reset_code_to_wire(code: DataStreamResetCode) -> u64 {
    STREAM_RESET
        .iter()
        .find(|(defined, _)| *defined == code)
        .map(|(_, value)| *value)
        .unwrap_or_else(|| {
            tracing::warn!(
                ?code,
                "stream reset code undefined in draft-18; sending INTERNAL_ERROR"
            );
            0x0
        })
}

/// §15.10.1.
pub(crate) fn termination_code_to_wire(code: TerminationErrorCode) -> u32 {
    match code {
        TerminationErrorCode::NoError => 0x0,
        // draft-18 removed TOO_MANY_REQUESTS with MAX_REQUEST_ID.
        TerminationErrorCode::InternalError | TerminationErrorCode::TooManyRequests => 0x1,
        TerminationErrorCode::Unauthorized => 0x2,
        TerminationErrorCode::ProtocolViolation => 0x3,
        TerminationErrorCode::InvalidRequestId => 0x4,
        TerminationErrorCode::DuplicateTrackAlias => 0x5,
        TerminationErrorCode::KeyValueFormattingError => 0x6,
        TerminationErrorCode::InvalidPath => 0x8,
        TerminationErrorCode::MalformedPath => 0x9,
        TerminationErrorCode::GoawayTimeout => 0x10,
        TerminationErrorCode::ControlMessageTimeout => 0x11,
        TerminationErrorCode::DataStreamTimeout => 0x12,
        TerminationErrorCode::AuthTokenCacheOverflow => 0x13,
        TerminationErrorCode::DuplicateAuthTokenAlias => 0x14,
        TerminationErrorCode::VersionNegotiationFailed => 0x15,
        TerminationErrorCode::MalformedAuthToken => 0x16,
        TerminationErrorCode::UnknownAuthTokenAlias => 0x17,
        TerminationErrorCode::ExpiredAuthToken => 0x18,
        TerminationErrorCode::InvalidAuthority => 0x19,
        TerminationErrorCode::MalformedAuthority => 0x1A,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PUBLISH_DONE, REQUEST_ERROR, STREAM_RESET, publish_done_code_from_wire,
        publish_done_code_to_wire, request_error_code_from_wire, request_error_code_to_wire,
        stream_reset_code_to_wire,
    };
    use crate::modules::moqt::control_plane::error_codes::{PublishDoneCode, RequestErrorCode};

    #[test]
    fn every_listed_code_round_trips() {
        for (code, value) in REQUEST_ERROR {
            // Act / Assert
            assert_eq!(request_error_code_to_wire(code), value, "{code:?}");
            assert_eq!(request_error_code_from_wire(value), code, "{value:#x}");
        }
        for (code, value) in PUBLISH_DONE {
            // Act / Assert
            assert_eq!(publish_done_code_to_wire(code), value, "{code:?}");
            assert_eq!(publish_done_code_from_wire(value), code, "{value:#x}");
        }
        for (code, value) in STREAM_RESET {
            // Act / Assert
            assert_eq!(stream_reset_code_to_wire(code), value, "{code:?}");
        }
    }

    #[test]
    fn draft14_request_codes_fold_into_draft18_ones() {
        // Act / Assert
        assert_eq!(
            request_error_code_to_wire(RequestErrorCode::NamespacePrefixUnknown),
            0x10
        );
        assert_eq!(
            request_error_code_to_wire(RequestErrorCode::NoObjects),
            0x11
        );
        assert_eq!(
            request_error_code_to_wire(RequestErrorCode::UnknownStatusInRange),
            0x0
        );
    }

    #[test]
    fn unknown_values_read_as_internal_error() {
        // Act / Assert
        assert_eq!(
            request_error_code_from_wire(0x7),
            RequestErrorCode::InternalError
        );
        assert_eq!(
            publish_done_code_from_wire(0x7),
            PublishDoneCode::InternalError
        );
    }
}

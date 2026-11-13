//! The values draft-18 gives the error codes (§15.10).

use crate::modules::moqt::control_plane::{
    constants::TerminationErrorCode,
    error_codes::{DataStreamResetCode, PublishDoneCode, RequestErrorCode},
};

/// Generates `$to_wire` and `$from_wire` from one list, which must name
/// every variant of `$code`. `A | B = value` sends `B` as `value`, which
/// reads as `A`.
macro_rules! wire_codes {
    ($code:ident: $to_wire:ident, $from_wire:ident {
        $($first:ident $(| $alias:ident)* = $value:literal),+ $(,)?
    }) => {
        pub(crate) fn $to_wire(code: $code) -> u64 {
            match code {
                $($code::$first $(| $code::$alias)* => $value),+
            }
        }

        /// §14: an unknown value reads as INTERNAL_ERROR.
        pub(crate) fn $from_wire(value: u64) -> $code {
            match value {
                $($value => $code::$first,)+
                _ => $code::InternalError,
            }
        }
    };
}

// §15.10.2. draft-18 numbers request errors once for every request kind. It
// has no value for the draft-14 codes after a `|`, which are sent as the
// closest broader code.
wire_codes! {
    RequestErrorCode: request_error_code_to_wire, request_error_code_from_wire {
        InternalError | UnknownStatusInRange = 0x0,
        Unauthorized = 0x1,
        Timeout = 0x2,
        NotSupported = 0x3,
        MalformedAuthToken = 0x4,
        ExpiredAuthToken = 0x5,
        GoingAway = 0x6,
        ExcessiveLoad = 0x9,
        TrackDoesNotExist | NamespacePrefixUnknown = 0x10,
        InvalidRange | NoObjects = 0x11,
        MalformedTrack = 0x12,
        DuplicateSubscription = 0x19,
        Uninterested = 0x20,
        NamespacePrefixOverlap = 0x30,
        NamespaceTooLarge = 0x31,
        InvalidJoiningRequestId = 0x32,
        UnsupportedExtension = 0x33,
        Redirect = 0x34,
    }
}

// §15.10.3.
wire_codes! {
    PublishDoneCode: publish_done_code_to_wire, publish_done_code_from_wire {
        InternalError = 0x0,
        Unauthorized = 0x1,
        TrackEnded = 0x2,
        SubscriptionEnded = 0x3,
        GoingAway = 0x4,
        TooFarBehind = 0x5,
        Expired = 0x6,
        UpdateFailed = 0x8,
        ExcessiveLoad = 0x9,
        MalformedTrack = 0x12,
    }
}

/// §15.10.4.
pub(crate) fn stream_reset_code_to_wire(code: DataStreamResetCode) -> u64 {
    match code {
        DataStreamResetCode::InternalError => 0x0,
        DataStreamResetCode::Cancelled => 0x1,
        DataStreamResetCode::DeliveryTimeout => 0x2,
        DataStreamResetCode::SessionClosed => 0x3,
        DataStreamResetCode::GoingAway => 0x4,
        DataStreamResetCode::TooFarBehind => 0x5,
        DataStreamResetCode::UnknownObjectStatus => 0x6,
        DataStreamResetCode::ExpiredAuthToken => 0x7,
        DataStreamResetCode::ExcessiveLoad => 0x9,
        DataStreamResetCode::MalformedTrack => 0x12,
    }
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
        publish_done_code_from_wire, publish_done_code_to_wire, request_error_code_from_wire,
        request_error_code_to_wire,
    };
    use crate::modules::moqt::control_plane::error_codes::{PublishDoneCode, RequestErrorCode};

    #[test]
    fn a_value_reads_as_the_code_it_is_sent_for() {
        // Act / Assert
        assert_eq!(
            request_error_code_from_wire(request_error_code_to_wire(RequestErrorCode::Redirect)),
            RequestErrorCode::Redirect
        );
        assert_eq!(
            publish_done_code_from_wire(publish_done_code_to_wire(PublishDoneCode::UpdateFailed)),
            PublishDoneCode::UpdateFailed
        );
    }

    #[test]
    fn draft14_request_codes_fold_into_draft18_ones() {
        // Act
        let sent = [
            RequestErrorCode::NamespacePrefixUnknown,
            RequestErrorCode::NoObjects,
            RequestErrorCode::UnknownStatusInRange,
        ]
        .map(request_error_code_to_wire);

        // Assert
        assert_eq!(sent, [0x10, 0x11, 0x0]);
        assert_eq!(
            sent.map(request_error_code_from_wire),
            [
                RequestErrorCode::TrackDoesNotExist,
                RequestErrorCode::InvalidRange,
                RequestErrorCode::InternalError,
            ]
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

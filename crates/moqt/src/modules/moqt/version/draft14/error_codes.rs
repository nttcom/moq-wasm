//! The values draft-14 gives the error codes (§13.1).

use crate::modules::moqt::control_plane::{
    constants::TerminationErrorCode,
    error_codes::{DataStreamResetCode, PublishDoneCode, RequestErrorCode, RequestKind},
};

type CodeTable = &'static [(RequestErrorCode, u64)];

const COMMON: CodeTable = &[
    (RequestErrorCode::InternalError, 0x0),
    (RequestErrorCode::Unauthorized, 0x1),
    (RequestErrorCode::Timeout, 0x2),
    (RequestErrorCode::NotSupported, 0x3),
];
const AUTH_TOKEN: CodeTable = &[
    (RequestErrorCode::MalformedAuthToken, 0x10),
    (RequestErrorCode::ExpiredAuthToken, 0x12),
];
const SUBSCRIBE: CodeTable = &[
    (RequestErrorCode::TrackDoesNotExist, 0x4),
    (RequestErrorCode::InvalidRange, 0x5),
];
const FETCH: CodeTable = &[
    (RequestErrorCode::TrackDoesNotExist, 0x4),
    (RequestErrorCode::InvalidRange, 0x5),
    (RequestErrorCode::NoObjects, 0x6),
    (RequestErrorCode::InvalidJoiningRequestId, 0x7),
    (RequestErrorCode::UnknownStatusInRange, 0x8),
    (RequestErrorCode::MalformedTrack, 0x9),
];
const UNINTERESTED: CodeTable = &[(RequestErrorCode::Uninterested, 0x4)];
const SUBSCRIBE_NAMESPACE: CodeTable = &[
    (RequestErrorCode::NamespacePrefixUnknown, 0x4),
    (RequestErrorCode::NamespacePrefixOverlap, 0x5),
];

fn request_error_table(kind: RequestKind) -> impl Iterator<Item = (RequestErrorCode, u64)> {
    let (specific, auth_token) = match kind {
        RequestKind::Subscribe | RequestKind::TrackStatus => (SUBSCRIBE, AUTH_TOKEN),
        RequestKind::Fetch => (FETCH, AUTH_TOKEN),
        RequestKind::Publish => (UNINTERESTED, &[][..]),
        RequestKind::PublishNamespace => (UNINTERESTED, AUTH_TOKEN),
        RequestKind::SubscribeNamespace => (SUBSCRIBE_NAMESPACE, AUTH_TOKEN),
    };
    COMMON.iter().chain(specific).chain(auth_token).copied()
}

/// Returns INTERNAL_ERROR (0x0) when `kind`'s `*_ERROR` message does not
/// define `code`.
pub fn request_error_code_to_wire(code: RequestErrorCode, kind: RequestKind) -> u64 {
    request_error_table(kind)
        .find(|(defined, _)| *defined == code)
        .map(|(_, value)| value)
        .unwrap_or_else(|| {
            tracing::warn!(
                ?code,
                ?kind,
                "error code undefined for request kind; sending INTERNAL_ERROR"
            );
            0x0
        })
}

/// Returns `InternalError` for a value `kind`'s `*_ERROR` message does not
/// define.
pub fn request_error_code_from_wire(value: u64, kind: RequestKind) -> RequestErrorCode {
    request_error_table(kind)
        .find(|(_, defined)| *defined == value)
        .map(|(code, _)| code)
        .unwrap_or(RequestErrorCode::InternalError)
}

const PUBLISH_DONE: [(PublishDoneCode, u64); 8] = [
    (PublishDoneCode::InternalError, 0x0),
    (PublishDoneCode::Unauthorized, 0x1),
    (PublishDoneCode::TrackEnded, 0x2),
    (PublishDoneCode::SubscriptionEnded, 0x3),
    (PublishDoneCode::GoingAway, 0x4),
    (PublishDoneCode::Expired, 0x5),
    (PublishDoneCode::TooFarBehind, 0x6),
    (PublishDoneCode::MalformedTrack, 0x7),
];

pub fn publish_done_code_to_wire(code: PublishDoneCode) -> u64 {
    PUBLISH_DONE
        .iter()
        .find(|(defined, _)| *defined == code)
        .map(|(_, value)| *value)
        .expect("PUBLISH_DONE lists every PublishDoneCode")
}

pub fn publish_done_code_from_wire(value: u64) -> Option<PublishDoneCode> {
    PUBLISH_DONE
        .iter()
        .find(|(_, defined)| *defined == value)
        .map(|(code, _)| *code)
}

pub(crate) fn data_stream_reset_code_to_wire(code: DataStreamResetCode) -> u64 {
    match code {
        DataStreamResetCode::InternalError => 0x0,
        DataStreamResetCode::Cancelled => 0x1,
        DataStreamResetCode::DeliveryTimeout => 0x2,
        DataStreamResetCode::SessionClosed => 0x3,
        // §2.5 resets the FETCH stream of a malformed track "with Status Code
        // MALFORMED_TRACK", which draft-14 defines only as FETCH_ERROR 0x9.
        DataStreamResetCode::MalformedTrack => 0x9,
    }
}

pub(crate) fn termination_code_to_wire(code: TerminationErrorCode) -> u32 {
    match code {
        TerminationErrorCode::NoError => 0x0,
        TerminationErrorCode::InternalError => 0x1,
        TerminationErrorCode::Unauthorized => 0x2,
        TerminationErrorCode::ProtocolViolation => 0x3,
        TerminationErrorCode::InvalidRequestId => 0x4,
        TerminationErrorCode::DuplicateTrackAlias => 0x5,
        TerminationErrorCode::KeyValueFormattingError => 0x6,
        TerminationErrorCode::TooManyRequests => 0x7,
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
        publish_done_code_from_wire, request_error_code_from_wire, request_error_code_to_wire,
        request_error_table,
    };
    use crate::modules::moqt::control_plane::error_codes::{RequestErrorCode, RequestKind};

    const ALL_KINDS: [RequestKind; 6] = [
        RequestKind::Subscribe,
        RequestKind::Publish,
        RequestKind::Fetch,
        RequestKind::TrackStatus,
        RequestKind::PublishNamespace,
        RequestKind::SubscribeNamespace,
    ];

    #[test]
    fn every_defined_request_error_code_round_trips_for_its_request_kind() {
        for kind in ALL_KINDS {
            for (code, value) in request_error_table(kind) {
                // Act / Assert
                assert_eq!(
                    request_error_code_to_wire(code, kind),
                    value,
                    "{code:?} {kind:?}"
                );
                assert_eq!(
                    request_error_code_from_wire(value, kind),
                    code,
                    "{value:#x} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn request_error_code_undefined_for_the_request_kind_is_sent_as_internal_error() {
        // Act / Assert
        assert_eq!(
            request_error_code_to_wire(RequestErrorCode::Uninterested, RequestKind::Subscribe),
            0x0
        );
        assert_eq!(
            request_error_code_to_wire(RequestErrorCode::MalformedAuthToken, RequestKind::Publish),
            0x0
        );
    }

    #[test]
    fn unknown_request_error_value_reads_as_internal_error() {
        // Act / Assert
        assert_eq!(
            request_error_code_from_wire(0x9, RequestKind::Subscribe),
            RequestErrorCode::InternalError
        );
    }

    #[test]
    fn unknown_publish_done_value_reads_as_none() {
        // Act / Assert
        assert_eq!(publish_done_code_from_wire(0x8), None);
    }
}

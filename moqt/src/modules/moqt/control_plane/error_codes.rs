use num_enum::IntoPrimitive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Subscribe,
    Publish,
    Fetch,
    TrackStatus,
    PublishNamespace,
    SubscribeNamespace,
}

type CodeTable = &'static [(RequestErrorCode, u64)];

/// Request error codes, draft-ietf-moq-transport-14 §13.1.2 and §13.1.4–§13.1.7.
/// draft-14 numbers them per error message, so the wire value depends on the
/// request kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestErrorCode {
    InternalError,
    Unauthorized,
    Timeout,
    NotSupported,
    TrackDoesNotExist,
    InvalidRange,
    NoObjects,
    InvalidJoiningRequestId,
    UnknownStatusInRange,
    MalformedTrack,
    MalformedAuthToken,
    ExpiredAuthToken,
    Uninterested,
    NamespacePrefixUnknown,
    NamespacePrefixOverlap,
}

impl RequestErrorCode {
    const COMMON: [(Self, u64); 4] = [
        (Self::InternalError, 0x0),
        (Self::Unauthorized, 0x1),
        (Self::Timeout, 0x2),
        (Self::NotSupported, 0x3),
    ];
    const AUTH_TOKEN: [(Self, u64); 2] = [
        (Self::MalformedAuthToken, 0x10),
        (Self::ExpiredAuthToken, 0x12),
    ];
    const SUBSCRIBE: [(Self, u64); 2] = [(Self::TrackDoesNotExist, 0x4), (Self::InvalidRange, 0x5)];
    const FETCH: [(Self, u64); 6] = [
        (Self::TrackDoesNotExist, 0x4),
        (Self::InvalidRange, 0x5),
        (Self::NoObjects, 0x6),
        (Self::InvalidJoiningRequestId, 0x7),
        (Self::UnknownStatusInRange, 0x8),
        (Self::MalformedTrack, 0x9),
    ];
    const UNINTERESTED: [(Self, u64); 1] = [(Self::Uninterested, 0x4)];
    const SUBSCRIBE_NAMESPACE: [(Self, u64); 2] = [
        (Self::NamespacePrefixUnknown, 0x4),
        (Self::NamespacePrefixOverlap, 0x5),
    ];

    fn table(kind: RequestKind) -> impl Iterator<Item = (Self, u64)> {
        let (specific, auth_token): (CodeTable, CodeTable) = match kind {
            RequestKind::Subscribe | RequestKind::TrackStatus => {
                (&Self::SUBSCRIBE, &Self::AUTH_TOKEN)
            }
            RequestKind::Fetch => (&Self::FETCH, &Self::AUTH_TOKEN),
            RequestKind::Publish => (&Self::UNINTERESTED, &[]),
            RequestKind::PublishNamespace => (&Self::UNINTERESTED, &Self::AUTH_TOKEN),
            RequestKind::SubscribeNamespace => (&Self::SUBSCRIBE_NAMESPACE, &Self::AUTH_TOKEN),
        };
        Self::COMMON
            .into_iter()
            .chain(specific.iter().copied())
            .chain(auth_token.iter().copied())
    }

    pub fn wire_value(self, kind: RequestKind) -> u64 {
        Self::table(kind)
            .find(|(code, _)| *code == self)
            .map(|(_, value)| value)
            .unwrap_or_else(|| {
                tracing::warn!(code = ?self, ?kind, "error code undefined for request kind; sending INTERNAL_ERROR");
                0x0
            })
    }

    pub fn from_wire(value: u64, kind: RequestKind) -> Self {
        Self::table(kind)
            .find(|(_, wire)| *wire == value)
            .map(|(code, _)| code)
            .unwrap_or(Self::InternalError)
    }
}

/// PUBLISH_DONE status codes, draft-ietf-moq-transport-14 §13.1.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive)]
#[repr(u64)]
pub enum PublishDoneCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
    TrackEnded = 0x2,
    SubscriptionEnded = 0x3,
    GoingAway = 0x4,
    Expired = 0x5,
    TooFarBehind = 0x6,
    MalformedTrack = 0x7,
}

/// Data stream reset codes, draft-ietf-moq-transport-14 §13.1.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive)]
#[repr(u64)]
pub enum DataStreamResetCode {
    InternalError = 0x0,
    Cancelled = 0x1,
    DeliveryTimeout = 0x2,
    SessionClosed = 0x3,
    // §2.5 resets fetch streams of a malformed track "with Status Code
    // MALFORMED_TRACK", which draft-14 only defines as FETCH_ERROR 0x9.
    MalformedTrack = 0x9,
}

#[cfg(test)]
mod tests {
    use super::{RequestErrorCode, RequestKind};

    const ALL_KINDS: [RequestKind; 6] = [
        RequestKind::Subscribe,
        RequestKind::Publish,
        RequestKind::Fetch,
        RequestKind::TrackStatus,
        RequestKind::PublishNamespace,
        RequestKind::SubscribeNamespace,
    ];

    #[test]
    fn every_defined_code_round_trips_for_its_request_kind() {
        for kind in ALL_KINDS {
            for (code, value) in RequestErrorCode::table(kind) {
                // Act / Assert
                assert_eq!(code.wire_value(kind), value, "{code:?} {kind:?}");
                assert_eq!(
                    RequestErrorCode::from_wire(value, kind),
                    code,
                    "{value:#x} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn code_undefined_for_the_request_kind_is_sent_as_internal_error() {
        // Act / Assert
        assert_eq!(
            RequestErrorCode::Uninterested.wire_value(RequestKind::Subscribe),
            0x0
        );
        assert_eq!(
            RequestErrorCode::MalformedAuthToken.wire_value(RequestKind::Publish),
            0x0
        );
    }

    #[test]
    fn unknown_value_is_read_as_internal_error() {
        // Act / Assert
        assert_eq!(
            RequestErrorCode::from_wire(0x9, RequestKind::Subscribe),
            RequestErrorCode::InternalError
        );
    }
}

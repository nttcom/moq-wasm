use moqt::publish::PublishDoneCode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishDoneReason {
    pub(crate) status_code: PublishDoneCode,
    pub(crate) error_reason: String,
}

impl PublishDoneReason {
    pub(crate) fn publisher_session_closed() -> Self {
        Self {
            status_code: PublishDoneCode::TrackEnded,
            error_reason: "publisher session closed".to_string(),
        }
    }

    pub(crate) fn subscription_ended() -> Self {
        Self {
            status_code: PublishDoneCode::SubscriptionEnded,
            error_reason: "end of subscription range reached".to_string(),
        }
    }

    pub(crate) fn malformed_track() -> Self {
        Self {
            status_code: PublishDoneCode::MalformedTrack,
            error_reason: "malformed track".to_string(),
        }
    }

    /// The subscriber's draft cannot carry an object's Group ID or Object ID
    /// (a draft-18 value of 2^62 or more toward draft-14), so no later object
    /// of the group can be sent; none of the draft-14 §13.1.3 codes says so.
    pub(crate) fn unencodable_object() -> Self {
        Self {
            status_code: PublishDoneCode::InternalError,
            error_reason: "object cannot be encoded for the subscriber's draft".to_string(),
        }
    }

    /// draft-18 §10.9.1: a REQUEST_UPDATE that failed ends its subscription.
    pub(crate) fn update_failed() -> Self {
        Self {
            status_code: PublishDoneCode::UpdateFailed,
            error_reason: "subscription update failed".to_string(),
        }
    }
}

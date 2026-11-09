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
}

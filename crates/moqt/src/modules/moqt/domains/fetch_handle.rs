use std::time::Duration;

use crate::{
    fetch::FetchOption,
    modules::{executor, moqt::control_plane::control_messages::messages::fetch_ok::FetchOk},
    object::{GroupOrder, Location},
};

#[derive(Clone, Debug)]
pub struct FetchHandle {
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub end_of_track: bool,
    pub end_location: Location,
    /// MAX_CACHE_DURATION in milliseconds (draft-14 §9.2.1.3) the FETCH_OK
    /// carried.
    pub max_cache_duration: Option<u64>,
    /// Set by a session that follows GOAWAY for a FETCH it can send again;
    /// `None` for a handle built outside a session.
    pub remainder: Option<FetchRemainder>,
}

/// What a FETCH sent again on the next connection asks for: the same track
/// from `start` on, up to the FETCH_OK's End Location.
#[derive(Clone, Debug)]
pub struct FetchRemainder {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) start: Location,
    pub(crate) subscriber_priority: u8,
    /// When the FILL_TIMEOUT of the first FETCH runs out, as
    /// `executor::now()`: a FETCH sent again asks only for what is left.
    pub(crate) fill_deadline: Option<Duration>,
}

impl FetchRemainder {
    /// For a FETCH sent now with `option`.
    pub(crate) fn new(
        track_namespace: String,
        track_name: String,
        start: Location,
        option: &FetchOption,
    ) -> Self {
        Self {
            track_namespace,
            track_name,
            start,
            subscriber_priority: option.subscriber_priority,
            fill_deadline: option
                .fill_timeout
                .map(|fill_timeout| executor::now() + fill_timeout),
        }
    }

    /// The FILL_TIMEOUT left for a FETCH sent again now, 0 once it ran out.
    pub(crate) fn fill_timeout_left(&self) -> Option<Duration> {
        self.fill_deadline
            .map(|deadline| deadline.saturating_sub(executor::now()))
    }
}

impl FetchHandle {
    pub(crate) fn new(fetch_ok: FetchOk) -> Self {
        Self {
            request_id: fetch_ok.request_id,
            group_order: fetch_ok.group_order,
            end_of_track: fetch_ok.end_of_track,
            end_location: fetch_ok.end_location,
            max_cache_duration: fetch_ok.max_cache_duration,
            remainder: None,
        }
    }
}

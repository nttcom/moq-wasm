use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::subscription_models::tracks::ForwardingPreference;
use crate::subscription_models::tracks::Track;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Status {
    Requesting,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    track: Track,
    priority: u8,
    group_order: GroupOrder,
    filter_type: FilterType,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
    end_object: Option<u64>,
    subscription_status: Status,
}

impl Subscription {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
        forwarding_preference: Option<ForwardingPreference>,
    ) -> Self {
        let track = Track::new(
            track_alias,
            track_namespace,
            track_name,
            forwarding_preference,
        );

        Self {
            track,
            priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            subscription_status: Status::Requesting,
        }
    }

    pub fn active(&mut self) -> bool {
        if self.is_active() {
            false
        } else {
            self.subscription_status = Status::Active;
            true
        }
    }

    pub fn is_active(&self) -> bool {
        self.subscription_status == Status::Active
    }

    pub fn is_requesting(&self) -> bool {
        self.subscription_status == Status::Requesting
    }

    pub fn set_forwarding_preference(&mut self, forwarding_preference: ForwardingPreference) {
        self.track.set_forwarding_preference(forwarding_preference);
    }

    pub fn get_track_namespace_and_name(&self) -> (Vec<String>, String) {
        self.track.get_track_namespace_and_name()
    }

    pub fn get_track_alias(&self) -> u64 {
        self.track.get_track_alias()
    }
}

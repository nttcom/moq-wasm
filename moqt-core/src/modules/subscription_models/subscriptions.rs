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

    pub fn activate(&mut self) -> bool {
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

    pub fn get_group_order(&self) -> GroupOrder {
        self.group_order
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};

    #[derive(Debug, Clone)]
    pub(crate) struct SubscriptionUtils {
        pub(crate) track_alias: u64,
        pub(crate) track_namespace: Vec<String>,
        pub(crate) track_name: String,
        pub(crate) subscriber_priority: u8,
        pub(crate) group_order: GroupOrder,
        pub(crate) filter_type: FilterType,
        pub(crate) start_group: Option<u64>,
        pub(crate) start_object: Option<u64>,
        pub(crate) end_group: Option<u64>,
        pub(crate) end_object: Option<u64>,
    }

    impl SubscriptionUtils {
        pub(crate) fn normal_variable() -> Self {
            let track_alias = 0;
            let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
            let track_name = "track_name".to_string();
            let subscriber_priority = 0;
            let group_order = GroupOrder::Ascending;
            let filter_type = FilterType::AbsoluteStart;
            let start_group = Some(0);
            let start_object = Some(0);
            let end_group = None;
            let end_object = None;

            SubscriptionUtils {
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                end_object,
            }
        }
    }
}

#[cfg(test)]
mod success {
    use super::*;
    use crate::subscription_models::tracks::ForwardingPreference;

    #[test]
    fn new() {
        let track_alias = 1;
        let track_namespace = vec!["namespace".to_string()];
        let track_name = "track_name".to_string();
        let priority = 1;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteRange;
        let start_group = Some(1);
        let start_object = Some(1);
        let end_group = Some(1);
        let end_object = Some(1);
        let forwarding_preference = Some(ForwardingPreference::Track);

        let subscription = Subscription::new(
            track_alias,
            track_namespace.clone(),
            track_name.clone(),
            priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
            end_object,
            forwarding_preference,
        );

        assert_eq!(subscription.track.get_track_alias(), track_alias);
        assert_eq!(
            subscription.get_track_namespace_and_name(),
            (track_namespace, track_name)
        );
        assert_eq!(subscription.priority, priority);
        assert_eq!(subscription.group_order, group_order);
        assert_eq!(subscription.filter_type, filter_type);
        assert_eq!(subscription.start_group, start_group);
        assert_eq!(subscription.start_object, start_object);
        assert_eq!(subscription.end_group, end_group);
        assert_eq!(subscription.end_object, end_object);
    }

    #[test]
    fn activate() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let mut subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace,
            variable.track_name,
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        let result = subscription.activate();
        assert!(result);

        let result = subscription.activate();
        assert!(!result);
    }

    #[test]
    fn is_active() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let mut subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace,
            variable.track_name,
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        assert!(!subscription.is_active());

        subscription.activate();
        assert!(subscription.is_active());
    }

    #[test]
    fn is_requesting() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let mut subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace,
            variable.track_name,
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        assert!(subscription.is_requesting());

        subscription.activate();
        assert!(!subscription.is_requesting());
    }

    #[test]
    fn get_track_namespace_and_name() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace.clone(),
            variable.track_name.clone(),
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        assert_eq!(
            subscription.get_track_namespace_and_name(),
            (variable.track_namespace, variable.track_name)
        );
    }

    #[test]
    fn get_track_alias() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace,
            variable.track_name,
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        assert_eq!(subscription.get_track_alias(), variable.track_alias);
    }

    #[test]
    fn get_group_order() {
        let variable = test_utils::SubscriptionUtils::normal_variable();

        let subscription = Subscription::new(
            variable.track_alias,
            variable.track_namespace,
            variable.track_name,
            variable.subscriber_priority,
            variable.group_order,
            variable.filter_type,
            variable.start_group,
            variable.start_object,
            variable.end_group,
            variable.end_object,
            None,
        );

        assert_eq!(subscription.get_group_order(), variable.group_order);
    }
}

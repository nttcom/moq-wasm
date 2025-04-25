pub mod nodes;
use super::range::{ObjectRange, ObjectStart};
use crate::{
    messages::control_messages::{group_order::GroupOrder, subscribe::FilterType},
    models::tracks::{ForwardingPreference, Track},
};

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
    requested_object_range: ObjectRange,
    actual_object_start: Option<ObjectStart>,
    status: Status,
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
        forwarding_preference: Option<ForwardingPreference>,
    ) -> Self {
        let track = Track::new(
            track_alias,
            track_namespace,
            track_name,
            forwarding_preference,
        );

        let requested_object_range = ObjectRange::new(start_group, start_object, end_group, None);

        Self {
            track,
            priority,
            group_order,
            filter_type,
            requested_object_range,
            actual_object_start: None,
            status: Status::Requesting,
        }
    }

    pub fn activate(&mut self) -> bool {
        if self.is_active() {
            false
        } else {
            self.status = Status::Active;
            true
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == Status::Active
    }

    pub fn is_requesting(&self) -> bool {
        self.status == Status::Requesting
    }

    pub fn get_filter_type(&self) -> FilterType {
        self.filter_type
    }

    pub fn get_requested_object_range(&self) -> ObjectRange {
        self.requested_object_range.clone()
    }

    pub fn set_forwarding_preference(&mut self, forwarding_preference: ForwardingPreference) {
        self.track.set_forwarding_preference(forwarding_preference);
    }

    pub fn get_forwarding_preference(&self) -> Option<ForwardingPreference> {
        self.track.get_forwarding_preference()
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

    pub fn set_stream_id(&mut self, group_id: u64, subgroup_id: u64, stream_id: u64) {
        self.track.set_stream_id(group_id, subgroup_id, stream_id);
    }

    pub fn get_all_group_ids(&self) -> Vec<u64> {
        let mut group_ids = self.track.get_all_group_ids();
        group_ids.sort_unstable();
        group_ids
    }

    pub fn get_subgroup_ids_for_group(&self, group_id: u64) -> Vec<u64> {
        let mut subgroup_ids = self.track.get_subgroup_ids_for_group(group_id);
        subgroup_ids.sort_unstable();
        subgroup_ids
    }

    pub fn get_stream_id_for_subgroup(&self, group_id: u64, subgroup_id: u64) -> Option<u64> {
        self.track.get_stream_id_for_subgroup(group_id, subgroup_id)
    }

    pub fn set_actual_object_start(&mut self, actual_object_start: ObjectStart) {
        self.actual_object_start = Some(actual_object_start);
    }

    pub fn get_actual_object_start(&self) -> Option<ObjectStart> {
        self.actual_object_start.clone()
    }
}

#[cfg(test)]
pub(crate) mod test_helper_fn {
    use crate::messages::control_messages::{group_order::GroupOrder, subscribe::FilterType};

    #[derive(Debug, Clone)]
    pub(crate) struct SubscriptionVariables {
        pub(crate) track_alias: u64,
        pub(crate) track_namespace: Vec<String>,
        pub(crate) track_name: String,
        pub(crate) subscriber_priority: u8,
        pub(crate) group_order: GroupOrder,
        pub(crate) filter_type: FilterType,
        pub(crate) start_group: Option<u64>,
        pub(crate) start_object: Option<u64>,
        pub(crate) end_group: Option<u64>,
    }

    pub(crate) fn common_subscription_variable() -> SubscriptionVariables {
        let track_alias = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();
        let subscriber_priority = 0;
        let group_order = GroupOrder::Ascending;
        let filter_type = FilterType::AbsoluteStart;
        let start_group = Some(0);
        let start_object = Some(0);
        let end_group = None;

        SubscriptionVariables {
            track_alias,
            track_namespace,
            track_name,
            subscriber_priority,
            group_order,
            filter_type,
            start_group,
            start_object,
            end_group,
        }
    }
}

#[cfg(test)]
mod success {
    use crate::{
        messages::control_messages::{group_order::GroupOrder, subscribe::FilterType},
        models::{
            range::ObjectStart,
            subscriptions::{test_helper_fn, Subscription},
            tracks::ForwardingPreference,
        },
    };

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
        let forwarding_preference = Some(ForwardingPreference::Subgroup);

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
        assert_eq!(
            subscription.requested_object_range.start_group_id(),
            start_group
        );
        assert_eq!(
            subscription.requested_object_range.start_object_id(),
            start_object
        );
        assert_eq!(
            subscription.requested_object_range.end_group_id(),
            end_group
        );
    }

    #[test]
    fn activate() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        let result = subscription.activate();
        assert!(result);

        let result = subscription.activate();
        assert!(!result);
    }

    #[test]
    fn is_active() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert!(!subscription.is_active());

        subscription.activate();
        assert!(subscription.is_active());
    }

    #[test]
    fn is_requesting() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert!(subscription.is_requesting());

        subscription.activate();
        assert!(!subscription.is_requesting());
    }

    #[test]
    fn get_filter_type() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert_eq!(subscription.get_filter_type(), variable.filter_type);
    }

    #[test]
    fn get_track_namespace_and_name() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert_eq!(
            subscription.get_track_namespace_and_name(),
            (variable.track_namespace, variable.track_name)
        );
    }

    #[test]
    fn get_track_alias() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert_eq!(subscription.get_track_alias(), variable.track_alias);
    }

    #[test]
    fn get_group_order() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        assert_eq!(subscription.get_group_order(), variable.group_order);
    }

    #[test]
    fn set_and_get_forwarding_preference() {
        let variable = test_helper_fn::common_subscription_variable();

        let forwarding_preference = ForwardingPreference::Subgroup;

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
            None,
        );

        subscription.set_forwarding_preference(forwarding_preference.clone());

        let result_forwarding_preference = subscription.get_forwarding_preference().unwrap();

        assert_eq!(result_forwarding_preference, forwarding_preference);
    }

    #[test]
    fn get_stream_id_for_group() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        let group_id = 0;
        let subgroup_ids = vec![0, 1, 2];
        let stream_ids = vec![3, 4, 5];

        subscription.set_stream_id(group_id, subgroup_ids[0], stream_ids[0]);
        subscription.set_stream_id(group_id, subgroup_ids[1], stream_ids[1]);
        subscription.set_stream_id(group_id, subgroup_ids[2], stream_ids[2]);

        let result_subgroup_ids = subscription.get_subgroup_ids_for_group(group_id);

        assert_eq!(result_subgroup_ids, subgroup_ids);

        let result_stream_id = vec![
            subscription
                .get_stream_id_for_subgroup(group_id, result_subgroup_ids[0])
                .unwrap(),
            subscription
                .get_stream_id_for_subgroup(group_id, result_subgroup_ids[1])
                .unwrap(),
            subscription
                .get_stream_id_for_subgroup(group_id, result_subgroup_ids[2])
                .unwrap(),
        ];

        assert_eq!(result_stream_id, stream_ids);
    }

    #[test]
    fn get_requested_object_range() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        let result = subscription.get_requested_object_range();

        assert_eq!(result.start_group_id(), variable.start_group);
        assert_eq!(result.start_object_id(), variable.start_object);
        assert_eq!(result.end_group_id(), variable.end_group);
    }

    #[test]
    fn set_actual_object_start() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        let start_group = 1;
        let start_object = 1;

        subscription.set_actual_object_start(ObjectStart::new(start_group, start_object));

        let result = subscription.get_actual_object_start().unwrap();

        assert_eq!(result.group_id(), start_group);
        assert_eq!(result.object_id(), start_object);
    }

    #[test]
    fn get_actual_object_start() {
        let variable = test_helper_fn::common_subscription_variable();

        let start_group = 1;
        let start_object = 1;

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
            None,
        );

        subscription.set_actual_object_start(ObjectStart::new(start_group, start_object));

        let result = subscription.get_actual_object_start().unwrap();

        assert_eq!(result.group_id(), start_group);
        assert_eq!(result.object_id(), start_object);
    }

    #[test]
    fn get_all_group_ids() {
        let variable = test_helper_fn::common_subscription_variable();

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
            None,
        );

        let group_ids = vec![0, 1, 2];

        subscription.set_stream_id(group_ids[0], 0, 0);
        subscription.set_stream_id(group_ids[1], 0, 0);
        subscription.set_stream_id(group_ids[2], 0, 0);

        let result = subscription.get_all_group_ids();

        assert_eq!(result, group_ids);
    }
}

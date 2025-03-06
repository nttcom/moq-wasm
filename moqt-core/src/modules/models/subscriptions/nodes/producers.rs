use anyhow::{bail, Result};
use std::collections::HashMap;

use crate::{
    messages::control_messages::subscribe::{FilterType, GroupOrder},
    models::{
        range::Range,
        subscriptions::{nodes::registry::SubscriptionNodeRegistry, Subscription},
        tracks::ForwardingPreference,
    },
};

type SubscribeId = u64;
type TrackNamespace = Vec<String>;
type TrackAlias = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Producer {
    max_subscriber_id: u64,
    announcing_namespaces: Vec<TrackNamespace>,
    subscribed_namespace_prefixes: Vec<TrackNamespace>,
    subscriptions: HashMap<SubscribeId, Subscription>,
}

impl Producer {
    pub fn new(max_subscriber_id: u64) -> Self {
        Producer {
            max_subscriber_id,
            announcing_namespaces: Vec::new(),
            subscribed_namespace_prefixes: Vec::new(),
            subscriptions: HashMap::new(),
        }
    }
}

impl SubscriptionNodeRegistry for Producer {
    fn set_subscription(
        &mut self,
        subscribe_id: SubscribeId,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Result<()> {
        // Publisher can define forwarding preference when it publishes track.
        let subscription = Subscription::new(
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
            None,
        );

        self.subscriptions.insert(subscribe_id, subscription);

        Ok(())
    }

    fn get_subscription(&self, subscribe_id: SubscribeId) -> Result<Option<Subscription>> {
        Ok(self.subscriptions.get(&subscribe_id).cloned())
    }

    fn get_subscribe_id(
        &self,
        track_namespace: TrackNamespace,
        track_name: String,
    ) -> Result<Option<SubscribeId>> {
        Ok(self
            .subscriptions
            .iter()
            .find(|(_, subscription)| {
                subscription.get_track_namespace_and_name()
                    == (track_namespace.clone(), track_name.clone())
            })
            .map(|(subscribe_id, _)| *subscribe_id))
    }

    fn get_track_alias(&self, subscribe_id: SubscribeId) -> Result<Option<TrackAlias>> {
        Ok(self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_track_alias()))
    }

    fn get_subscribe_id_by_track_alias(
        &self,
        track_alias: TrackAlias,
    ) -> Result<Option<SubscribeId>> {
        Ok(self
            .subscriptions
            .iter()
            .find(|(_, subscription)| subscription.get_track_alias() == track_alias)
            .map(|(subscribe_id, _)| *subscribe_id))
    }

    fn has_track(&self, track_namespace: TrackNamespace, track_name: String) -> bool {
        self.subscriptions.values().any(|subscription| {
            subscription.get_track_namespace_and_name()
                == (track_namespace.clone(), track_name.clone())
        })
    }

    fn activate_subscription(&mut self, subscribe_id: SubscribeId) -> Result<bool> {
        let subscription = self.subscriptions.get_mut(&subscribe_id).unwrap();
        let is_activated = subscription.activate();

        Ok(is_activated)
    }

    fn is_requesting(&self, subscribe_id: SubscribeId) -> bool {
        self.subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.is_requesting())
            .unwrap_or(false)
    }

    fn delete_subscription(&mut self, subscribe_id: SubscribeId) -> Result<()> {
        self.subscriptions.remove(&subscribe_id);
        Ok(())
    }

    fn set_forwarding_preference(
        &mut self,
        subscribe_id: SubscribeId,
        forwarding_preference: ForwardingPreference,
    ) -> Result<()> {
        let subscription = self.subscriptions.get_mut(&subscribe_id).unwrap();
        subscription.set_forwarding_preference(forwarding_preference);

        Ok(())
    }

    fn get_forwarding_preference(
        &self,
        subscribe_id: SubscribeId,
    ) -> Result<Option<ForwardingPreference>> {
        unimplemented!("subscribe_id: {}", subscribe_id)
    }

    fn get_filter_type(&self, subscribe_id: SubscribeId) -> Result<Option<FilterType>> {
        let filter_type = self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_filter_type());

        Ok(filter_type)
    }

    fn get_requested_range(&self, subscribe_id: SubscribeId) -> Result<Option<Range>> {
        let requested_range = self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_requested_range());

        Ok(requested_range)
    }

    fn get_absolute_start(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)> {
        let range = self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_requested_range())
            .unwrap();

        let start_group = range.start_group();
        let start_object = range.start_object();

        Ok((start_group, start_object))
    }

    fn get_absolute_end(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)> {
        let range = self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_requested_range())
            .unwrap();

        let end_group = range.end_group();
        let end_object = range.end_object();

        Ok((end_group, end_object))
    }

    fn set_stream_id_to_group(
        &mut self,
        subscribe_id: SubscribeId,
        group_id: u64,
        stream_id: u64,
    ) -> Result<()> {
        let subscription = self.subscriptions.get_mut(&subscribe_id).unwrap();
        subscription.set_stream_id_to_group(group_id, stream_id);

        Ok(())
    }

    fn get_stream_ids_from_group(
        &self,
        subscribe_id: SubscribeId,
        group_id: u64,
    ) -> Result<Vec<u64>> {
        let subscription = self.subscriptions.get(&subscribe_id).unwrap();
        let stream_ids = subscription.get_stream_ids_from_group(group_id);

        Ok(stream_ids.clone())
    }

    fn is_subscribe_id_unique(&self, subscribe_id: SubscribeId) -> bool {
        !self.subscriptions.contains_key(&subscribe_id)
    }

    fn is_subscribe_id_less_than_max(&self, subscribe_id: SubscribeId) -> bool {
        subscribe_id < self.max_subscriber_id
    }

    fn is_track_alias_unique(&self, track_alias: TrackAlias) -> bool {
        !self
            .subscriptions
            .values()
            .any(|subscription| subscription.get_track_alias() == track_alias)
    }

    fn create_valid_track_alias(&self) -> Result<TrackAlias> {
        let mut track_alias = 0;

        while !self.is_track_alias_unique(track_alias) {
            track_alias += 1;
        }

        Ok(track_alias)
    }

    fn create_latest_subscribe_id_and_track_alias(&self) -> Result<(SubscribeId, TrackAlias)> {
        unimplemented!()
    }

    fn set_namespace(&mut self, namespace: TrackNamespace) -> Result<()> {
        if self.announcing_namespaces.contains(&namespace) {
            bail!("Namespace already exists.");
        }

        self.announcing_namespaces.push(namespace);

        Ok(())
    }

    fn get_namespaces(&self) -> Result<&Vec<TrackNamespace>> {
        Ok(&self.announcing_namespaces)
    }

    fn has_namespace(&self, namespace: TrackNamespace) -> bool {
        self.announcing_namespaces.contains(&namespace)
    }

    fn delete_namespace(&mut self, namespace: TrackNamespace) -> Result<()> {
        if let Some(index) = self
            .announcing_namespaces
            .iter()
            .position(|x| x == &namespace)
        {
            self.announcing_namespaces.remove(index);
        }

        Ok(())
    }

    fn set_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()> {
        // Compare the given prefix with the prefixes that have already been registered from the beginning,
        // and if the given prefix matches the first half of the already registered prefix, or if the already
        // registered prefix matches the first half of the given prefix, an error will occur.
        // For example, if ["aaa", bbb] is already registered, ["aaa"] cannot be registered.
        for prefix in &self.subscribed_namespace_prefixes {
            let mut is_same_halfway = true;
            for (index, prefix_element) in prefix.iter().enumerate() {
                if index >= namespace_prefix.len() {
                    break;
                }

                if prefix_element != &namespace_prefix[index] {
                    is_same_halfway = false;
                    break;
                }
            }

            if is_same_halfway {
                bail!("Namespace prefix already exists.");
            }
        }

        self.subscribed_namespace_prefixes.push(namespace_prefix);

        Ok(())
    }

    fn get_namespace_prefixes(&self) -> Result<&Vec<TrackNamespace>> {
        Ok(&self.subscribed_namespace_prefixes)
    }

    fn delete_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()> {
        if let Some(index) = self
            .subscribed_namespace_prefixes
            .iter()
            .position(|x| x == &namespace_prefix)
        {
            self.subscribed_namespace_prefixes.remove(index);
        }

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_helper_fn {

    use crate::models::subscriptions::nodes::producers::{
        FilterType, GroupOrder, Producer, TrackNamespace,
    };

    #[derive(Debug, Clone)]
    pub(crate) struct SubscriptionVariables {
        pub(crate) producer: Producer,
        pub(crate) subscribe_id: u64,
        pub(crate) track_alias: u64,
        pub(crate) track_namespace: TrackNamespace,
        pub(crate) track_name: String,
        pub(crate) subscriber_priority: u8,
        pub(crate) group_order: GroupOrder,
        pub(crate) filter_type: FilterType,
        pub(crate) start_group: Option<u64>,
        pub(crate) start_object: Option<u64>,
        pub(crate) end_group: Option<u64>,
        pub(crate) end_object: Option<u64>,
    }

    pub(crate) fn common_subscription_variable(subscribe_id: u64) -> SubscriptionVariables {
        let producer = Producer::new(10);
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

        SubscriptionVariables {
            producer,
            subscribe_id,
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

#[cfg(test)]
mod success {
    use crate::models::{
        subscriptions::{
            nodes::{
                producers::{test_helper_fn, Producer},
                registry::SubscriptionNodeRegistry,
            },
            Subscription,
        },
        tracks::ForwardingPreference,
    };

    #[test]
    fn set_subscription() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let result = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace,
            variables.track_name,
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn get_subscription() {
        let subscribe_id = 0;
        let variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let mut variables_clone = variables.clone();
        let _ = variables_clone.producer.set_subscription(
            variables_clone.subscribe_id,
            variables_clone.track_alias,
            variables_clone.track_namespace,
            variables_clone.track_name,
            variables_clone.subscriber_priority,
            variables_clone.group_order,
            variables_clone.filter_type,
            variables_clone.start_group,
            variables_clone.start_object,
            variables_clone.end_group,
            variables_clone.end_object,
        );

        let subscription = variables_clone
            .producer
            .get_subscription(variables.clone().subscribe_id)
            .unwrap();

        let expected_subscription = Some(Subscription::new(
            variables.track_alias,
            variables.track_namespace,
            variables.track_name,
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
            None,
        ));

        assert_eq!(subscription, expected_subscription);
    }

    #[test]
    fn get_subscribe_id() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let expected_subscribe_id = variables.subscribe_id;

        let result_subscribe_id = variables
            .producer
            .get_subscribe_id(variables.track_namespace, variables.track_name)
            .unwrap()
            .unwrap();

        assert_eq!(result_subscribe_id, expected_subscribe_id);
    }

    #[test]
    fn get_track_alias() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let expected_track_alias = variables.track_alias;

        let result_track_alias = variables
            .producer
            .get_track_alias(variables.subscribe_id)
            .unwrap()
            .unwrap();

        assert_eq!(result_track_alias, expected_track_alias);
    }

    #[test]
    fn get_subscribe_id_by_track_alias() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let expected_subscribe_id = variables.subscribe_id;

        let result_subscribe_id = variables
            .producer
            .get_subscribe_id_by_track_alias(variables.track_alias)
            .unwrap()
            .unwrap();

        assert_eq!(result_subscribe_id, expected_subscribe_id);
    }

    #[test]
    fn has_track() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .has_track(variables.track_namespace, variables.track_name);

        assert!(result);
    }

    #[test]
    fn activate_subscription() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .activate_subscription(variables.subscribe_id)
            .unwrap();

        assert!(result);
    }

    #[test]
    fn is_requesting() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables.producer.is_requesting(variables.subscribe_id);

        assert!(result);
    }

    #[test]
    fn delete_subscription() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .delete_subscription(variables.subscribe_id);

        assert!(result.is_ok());
    }

    #[test]
    fn set_forwarding_preference() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let forwarding_preference = ForwardingPreference::Datagram;

        let result = variables
            .producer
            .set_forwarding_preference(variables.subscribe_id, forwarding_preference);

        assert!(result.is_ok());
    }

    #[test]
    #[should_panic]
    fn get_forwarding_preference() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let forwarding_preference = ForwardingPreference::Datagram;

        let _ = variables
            .producer
            .set_forwarding_preference(variables.subscribe_id, forwarding_preference.clone());

        let _ = variables
            .producer
            .get_forwarding_preference(variables.subscribe_id)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn get_filter_type() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result_filter_type = variables
            .producer
            .get_filter_type(variables.subscribe_id)
            .unwrap()
            .unwrap();

        assert_eq!(result_filter_type, variables.filter_type);
    }

    #[test]
    fn get_requested_range() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result_range = variables
            .producer
            .get_requested_range(variables.subscribe_id)
            .unwrap()
            .unwrap();

        assert_eq!(result_range.start_group(), variables.start_group);
        assert_eq!(result_range.start_object(), variables.start_object);
        assert_eq!(result_range.end_group(), variables.end_group);
        assert_eq!(result_range.end_object(), variables.end_object);
    }

    #[test]
    fn get_absolute_start() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .get_absolute_start(variables.subscribe_id)
            .unwrap();

        let expected_result = (variables.start_group, variables.start_object);

        assert_eq!(result, expected_result);
    }

    #[test]
    fn get_absolute_end() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .get_absolute_end(variables.subscribe_id)
            .unwrap();

        let expected_result = (variables.end_group, variables.end_object);

        assert_eq!(result, expected_result);
    }

    #[test]
    fn is_subscribe_id_unique() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .is_subscribe_id_unique(variables.subscribe_id + 1);

        assert!(result);

        let result = variables
            .producer
            .is_subscribe_id_unique(variables.subscribe_id);

        assert!(!result);
    }

    #[test]
    fn is_subscribe_id_less_than_max() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .is_subscribe_id_less_than_max(variables.subscribe_id);

        assert!(result);

        let result = variables
            .producer
            .is_subscribe_id_less_than_max(variables.subscribe_id + 10);

        assert!(!result);
    }

    #[test]
    fn is_track_alias_unique() {
        let mut variables = test_helper_fn::common_subscription_variable(0);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables
            .producer
            .is_track_alias_unique(variables.track_alias + 1);

        assert!(result);

        let result = variables
            .producer
            .is_track_alias_unique(variables.track_alias);

        assert!(!result);
    }

    #[test]
    fn create_valid_track_alias() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.producer.set_subscription(
            variables.subscribe_id,
            variables.track_alias,
            variables.track_namespace.clone(),
            variables.track_name.clone(),
            variables.subscriber_priority,
            variables.group_order,
            variables.filter_type,
            variables.start_group,
            variables.start_object,
            variables.end_group,
            variables.end_object,
        );

        let result = variables.producer.create_valid_track_alias();

        assert!(result.is_ok());

        let expected_alias = variables.track_alias + 1;

        assert_eq!(result.unwrap(), expected_alias);
    }

    #[test]
    #[should_panic]
    fn create_latest_subscribe_id_and_track_alias() {
        let producer = Producer::new(10);

        let _ = producer.create_latest_subscribe_id_and_track_alias();
    }

    #[test]
    fn set_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut producer = Producer::new(10);

        let result = producer.set_namespace(namespace);

        assert!(result.is_ok());
    }

    #[test]
    fn get_namespaces() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut producer = Producer::new(10);

        let _ = producer.set_namespace(namespace.clone());

        let expected_result = &vec![namespace];

        let result = producer.get_namespaces().unwrap();

        assert_eq!(result, expected_result);
    }

    #[test]
    fn has_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut producer = Producer::new(10);

        let _ = producer.set_namespace(namespace.clone());

        let result = producer.has_namespace(namespace);

        assert!(result);
    }

    #[test]
    fn delete_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut producer = Producer::new(10);

        let _ = producer.set_namespace(namespace.clone());

        let result = producer.delete_namespace(namespace);
        assert!(result.is_ok());

        let namespaces = producer.get_namespaces().unwrap();
        assert!(namespaces.is_empty());
    }

    #[test]
    fn set_namespace_prefix() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut producer = Producer::new(10);

        let result = producer.set_namespace_prefix(namespace_prefix);

        assert!(result.is_ok());
    }

    #[test]
    fn get_namespace_prefixes() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut producer = Producer::new(10);

        let _ = producer.set_namespace_prefix(namespace_prefix.clone());

        let expected_result = &vec![namespace_prefix];

        let result = producer.get_namespace_prefixes().unwrap();

        assert_eq!(result, expected_result);
    }

    #[test]
    fn delete_namespace_prefix() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut producer = Producer::new(10);

        let _ = producer.set_namespace_prefix(namespace_prefix.clone());

        let result = producer.delete_namespace_prefix(namespace_prefix);
        assert!(result.is_ok());

        let namespace_prefixes = producer.get_namespace_prefixes().unwrap();
        assert!(namespace_prefixes.is_empty());
    }
}

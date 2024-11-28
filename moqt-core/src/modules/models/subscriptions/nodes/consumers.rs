use anyhow::{bail, Result};
use std::collections::HashMap;

use crate::{
    messages::control_messages::subscribe::{FilterType, GroupOrder},
    models::{
        subscriptions::{nodes::registry::SubscriptionNodeRegistry, Subscription},
        tracks::ForwardingPreference,
    },
};

type SubscribeId = u64;
type TrackNamespace = Vec<String>;
type TrackAlias = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consumer {
    max_subscriber_id: u64,
    announced_namespaces: Vec<TrackNamespace>,
    subscribing_namespace_prefixes: Vec<TrackNamespace>,
    subscriptions: HashMap<SubscribeId, Subscription>,
    latest_subscribe_id: u64,
}

impl Consumer {
    pub fn new(max_subscriber_id: u64) -> Self {
        Consumer {
            max_subscriber_id,
            announced_namespaces: Vec::new(),
            subscribing_namespace_prefixes: Vec::new(),
            subscriptions: HashMap::new(),
            latest_subscribe_id: 0,
        }
    }
}

impl SubscriptionNodeRegistry for Consumer {
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
        // Subscriber cannot define forwarding preference until it receives object message.
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

        self.latest_subscribe_id = subscribe_id + 1;
        self.subscriptions.insert(subscribe_id, subscription);

        Ok(())
    }

    fn get_subscription(&self, subscribe_id: SubscribeId) -> Result<Option<Subscription>> {
        Ok(self.subscriptions.get(&subscribe_id).cloned())
    }

    fn get_subscription_by_full_track_name(
        &self,
        track_namespace: TrackNamespace,
        track_name: String,
    ) -> Result<Option<Subscription>> {
        Ok(self
            .subscriptions
            .values()
            .find(|subscription| {
                subscription.get_track_namespace_and_name()
                    == (track_namespace.clone(), track_name.clone())
            })
            .cloned())
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
        let forwarding_preference = self
            .subscriptions
            .get(&subscribe_id)
            .map(|subscription| subscription.get_forwarding_preference().unwrap());
        Ok(forwarding_preference)
    }

    fn get_filter_type(&self, subscribe_id: SubscribeId) -> Result<FilterType> {
        unimplemented!("subscribe_id: {}", subscribe_id)
    }

    fn get_absolute_start(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)> {
        unimplemented!("subscribe_id: {}", subscribe_id)
    }

    fn get_absolute_end(&self, subscribe_id: SubscribeId) -> Result<(Option<u64>, Option<u64>)> {
        unimplemented!("subscribe_id: {}", subscribe_id)
    }

    fn is_subscribe_id_valid(&self, subscribe_id: SubscribeId) -> bool {
        let is_less_than_max_subscribe_id = subscribe_id < self.max_subscriber_id;
        let is_unique = !self.subscriptions.contains_key(&subscribe_id);

        is_less_than_max_subscribe_id && is_unique
    }

    fn is_track_alias_valid(&self, track_alias: TrackAlias) -> bool {
        let is_unique = !self
            .subscriptions
            .values()
            .any(|subscription| subscription.get_track_alias() == track_alias);

        is_unique
    }

    fn create_valid_track_alias(&self) -> Result<TrackAlias> {
        unimplemented!()
    }

    // TODO: Separate this function into two functions.
    fn create_latest_subscribe_id_and_track_alias(&self) -> Result<(SubscribeId, TrackAlias)> {
        let subscribe_id = self.latest_subscribe_id;
        match self.is_subscribe_id_valid(subscribe_id) {
            false => {
                bail!("No available subscribe_id.");
            }
            true => {
                for track_alias in 0.. {
                    if !self
                        .subscriptions
                        .values()
                        .any(|subscription| subscription.get_track_alias() == track_alias)
                    {
                        return Ok((subscribe_id, track_alias));
                    }
                }

                bail!("No available track_alias.");
            }
        }
    }

    fn set_namespace(&mut self, namespace: TrackNamespace) -> Result<()> {
        if self.announced_namespaces.contains(&namespace) {
            bail!("Namespace already exists.");
        }

        self.announced_namespaces.push(namespace);

        Ok(())
    }

    fn get_namespaces(&self) -> Result<&Vec<TrackNamespace>> {
        Ok(&self.announced_namespaces)
    }

    fn has_namespace(&self, namespace: TrackNamespace) -> bool {
        self.announced_namespaces.contains(&namespace)
    }

    fn delete_namespace(&mut self, namespace: TrackNamespace) -> Result<()> {
        if let Some(index) = self
            .announced_namespaces
            .iter()
            .position(|x| x == &namespace)
        {
            self.announced_namespaces.remove(index);
        }

        Ok(())
    }
    fn set_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()> {
        if self
            .subscribing_namespace_prefixes
            .contains(&namespace_prefix)
        {
            bail!("Namespace prefix already exists.");
        }

        self.subscribing_namespace_prefixes.push(namespace_prefix);

        Ok(())
    }

    fn get_namespace_prefixes(&self) -> Result<&Vec<TrackNamespace>> {
        Ok(&self.subscribing_namespace_prefixes)
    }

    fn delete_namespace_prefix(&mut self, namespace_prefix: TrackNamespace) -> Result<()> {
        if let Some(index) = self
            .subscribing_namespace_prefixes
            .iter()
            .position(|x| x == &namespace_prefix)
        {
            self.subscribing_namespace_prefixes.remove(index);
        }

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_helper_fn {
    use crate::models::subscriptions::nodes::consumers::{
        Consumer, FilterType, GroupOrder, TrackNamespace,
    };

    #[derive(Debug, Clone)]
    pub(crate) struct SubscriptionVariables {
        pub(crate) consumer: Consumer,
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
        let consumer = Consumer::new(10);
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
            consumer,
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
                consumers::{test_helper_fn, Consumer},
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

        let result = variables.consumer.set_subscription(
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
        let _ = variables_clone.consumer.set_subscription(
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
            .consumer
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
    fn get_subscription_by_full_track_name() {
        let subscribe_id = 0;
        let variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let mut variables_clone = variables.clone();
        let _ = variables_clone.consumer.set_subscription(
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
            .consumer
            .get_subscription_by_full_track_name(
                variables.track_namespace.clone(),
                variables.track_name.clone(),
            )
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

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .get_subscribe_id(variables.track_namespace, variables.track_name)
            .unwrap()
            .unwrap();

        assert_eq!(result_subscribe_id, expected_subscribe_id);
    }

    #[test]
    fn has_track() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .has_track(variables.track_namespace, variables.track_name);

        assert!(result);
    }

    #[test]
    fn activate_subscription() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .activate_subscription(variables.subscribe_id)
            .unwrap();

        assert!(result);
    }

    #[test]
    fn is_requesting() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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

        let result = variables.consumer.is_requesting(variables.subscribe_id);

        assert!(result);
    }

    #[test]
    fn delete_subscription() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .delete_subscription(variables.subscribe_id);

        assert!(result.is_ok());
    }

    #[test]
    fn set_forwarding_preference() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .set_forwarding_preference(variables.subscribe_id, forwarding_preference);

        assert!(result.is_ok());
    }

    #[test]
    fn get_forwarding_preference() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .set_forwarding_preference(variables.subscribe_id, forwarding_preference.clone());

        let result = variables
            .consumer
            .get_forwarding_preference(variables.subscribe_id)
            .unwrap()
            .unwrap();

        print!("{:?}", result);

        assert_eq!(result, forwarding_preference);
    }

    #[test]
    #[should_panic]
    fn get_filter_type() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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

        let _ = variables
            .consumer
            .get_filter_type(variables.subscribe_id)
            .unwrap();
    }
    #[test]
    #[should_panic]
    fn get_absolute_start() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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

        let _ = variables
            .consumer
            .get_absolute_start(variables.subscribe_id)
            .unwrap();
    }

    #[test]
    #[should_panic]
    fn get_absolute_end() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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

        let _ = variables
            .consumer
            .get_absolute_end(variables.subscribe_id)
            .unwrap();
    }

    #[test]
    fn is_subscribe_id_valid() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .is_subscribe_id_valid(variables.subscribe_id);

        assert!(!result);
    }

    #[test]
    fn is_track_alias_valid() {
        let track_alias = 100;
        let mut variables = test_helper_fn::common_subscription_variable(0);

        let _ = variables.consumer.set_subscription(
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

        let result = variables.consumer.is_track_alias_valid(track_alias);

        assert!(result);
    }

    #[test]
    #[should_panic]
    fn create_valid_track_alias() {
        let consumer = Consumer::new(10);

        let _ = consumer.create_valid_track_alias().unwrap();
    }

    #[test]
    fn create_latest_subscribe_id_and_track_alias() {
        let subscribe_id = 0;
        let mut variables = test_helper_fn::common_subscription_variable(subscribe_id);

        let _ = variables.consumer.set_subscription(
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
            .consumer
            .create_latest_subscribe_id_and_track_alias();

        assert!(result.is_ok());

        let expected_id_and_alias = (variables.subscribe_id + 1, variables.track_alias + 1);

        assert_eq!(result.unwrap(), expected_id_and_alias);
    }

    #[test]
    fn set_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut consumer = Consumer::new(10);

        let result = consumer.set_namespace(namespace);

        assert!(result.is_ok());
    }

    #[test]
    fn get_namespaces() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut consumer = Consumer::new(10);

        let _ = consumer.set_namespace(namespace.clone());

        let expected_result = &vec![namespace];

        let result = consumer.get_namespaces().unwrap();

        assert_eq!(result, expected_result);
    }

    #[test]
    fn has_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut consumer = Consumer::new(10);

        let _ = consumer.set_namespace(namespace.clone());

        let result = consumer.has_namespace(namespace);

        assert!(result);
    }

    #[test]
    fn delete_namespace() {
        let namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let mut consumer = Consumer::new(10);

        let _ = consumer.set_namespace(namespace.clone());

        let result = consumer.delete_namespace(namespace);
        assert!(result.is_ok());

        let namespaces = consumer.get_namespaces().unwrap();
        assert!(namespaces.is_empty());
    }

    #[test]
    fn set_namespace_prefix() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut consumer = Consumer::new(10);

        let result = consumer.set_namespace_prefix(namespace_prefix);

        assert!(result.is_ok());
    }

    #[test]
    fn get_namespace_prefixes() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut consumer = Consumer::new(10);

        let _ = consumer.set_namespace_prefix(namespace_prefix.clone());

        let expected_result = &vec![namespace_prefix];

        let result = consumer.get_namespace_prefixes().unwrap();

        assert_eq!(result, expected_result);
    }

    #[test]
    fn delete_namespace_prefix() {
        let namespace_prefix = Vec::from(["test".to_string()]);
        let mut consumer = Consumer::new(10);

        let _ = consumer.set_namespace_prefix(namespace_prefix.clone());

        let result = consumer.delete_namespace_prefix(namespace_prefix);
        assert!(result.is_ok());

        let namespace_prefixes = consumer.get_namespace_prefixes().unwrap();
        assert!(namespace_prefixes.is_empty());
    }
}

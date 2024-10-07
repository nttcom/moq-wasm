use crate::messages::control_messages::subscribe::{FilterType, GroupOrder};
use crate::subscription_models::node_registory::SubscriptionNodeRegistory;
use crate::subscription_models::subscriptions::Subscription;
use anyhow::{bail, Result};
use std::collections::HashMap;

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

impl SubscriptionNodeRegistory for Producer {
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
        let activate = subscription.active();

        Ok(activate)
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

    fn is_within_max_subscribe_id(&self, subscribe_id: SubscribeId) -> bool {
        subscribe_id <= self.max_subscriber_id
    }

    fn is_subscribe_id_unique(&self, subscribe_id: SubscribeId) -> bool {
        !self.subscriptions.contains_key(&subscribe_id)
    }

    fn is_track_alias_unique(&self, track_alias: TrackAlias) -> bool {
        !self
            .subscriptions
            .values()
            .any(|subscription| subscription.get_track_alias() == track_alias)
    }

    fn find_unused_subscribe_id_and_track_alias(&self) -> Result<(SubscribeId, TrackAlias)> {
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
        if self
            .subscribed_namespace_prefixes
            .contains(&namespace_prefix)
        {
            bail!("Namespace prefix already exists.");
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

use anyhow::Result;
use std::collections::HashMap;

type SubscriberSessionId = usize;
type PublisherSessionId = usize;
type PublisherSubscribeId = u64;
type SubscriberSubscribeId = u64;

#[derive(Debug, Clone)]
pub(crate) struct PubSubRelation {
    pub(crate) records: HashMap<
        (PublisherSessionId, PublisherSubscribeId),
        Vec<(SubscriberSessionId, SubscriberSubscribeId)>,
    >,
}

impl PubSubRelation {
    pub(crate) fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    pub(crate) fn add_relation(
        &mut self,
        publisher_session_id: PublisherSessionId,
        publisher_subscribe_id: PublisherSubscribeId,
        subscriber_session_id: SubscriberSessionId,
        subscriber_subscribe_id: SubscriberSubscribeId,
    ) -> Result<()> {
        let key = (publisher_session_id, publisher_subscribe_id);
        let value = (subscriber_session_id, subscriber_subscribe_id);

        match self.records.get_mut(&key) {
            // If the key exists, add the value to the existing vector
            Some(subscribers) => {
                subscribers.push(value);
            }
            // If the key does not exist, create a new vector and insert the value
            None => {
                self.records.insert(key, vec![value]);
            }
        }

        Ok(())
    }

    pub(crate) fn get_subscribers(
        &self,
        publisher_session_id: PublisherSessionId,
        publisher_subscribe_id: PublisherSubscribeId,
    ) -> Option<&Vec<(SubscriberSessionId, SubscriberSubscribeId)>> {
        let key = (publisher_session_id, publisher_subscribe_id);
        self.records.get(&key)
    }

    // TODO: Define the behavior if the last subscriber unsubscribes from the track
    // fn delete_relation
}

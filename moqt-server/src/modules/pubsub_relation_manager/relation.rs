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
        upstream_session_id: PublisherSessionId,
        upstream_subscribe_id: PublisherSubscribeId,
        downstream_session_id: SubscriberSessionId,
        downstream_subscribe_id: SubscriberSubscribeId,
    ) -> Result<()> {
        let key = (upstream_session_id, upstream_subscribe_id);
        let value = (downstream_session_id, downstream_subscribe_id);

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
        upstream_session_id: PublisherSessionId,
        upstream_subscribe_id: PublisherSubscribeId,
    ) -> Option<&Vec<(SubscriberSessionId, SubscriberSubscribeId)>> {
        let key = (upstream_session_id, upstream_subscribe_id);
        self.records.get(&key)
    }

    pub(crate) fn get_publisher(
        &self,
        downstream_session_id: SubscriberSessionId,
        downstream_subscribe_id: SubscriberSubscribeId,
    ) -> Option<(PublisherSessionId, PublisherSubscribeId)> {
        for ((upstream_session_id, upstream_subscribe_id), subscribers) in &self.records {
            for (session_id, subscribe_id) in subscribers {
                if *session_id == downstream_session_id && *subscribe_id == downstream_subscribe_id
                {
                    return Some((*upstream_session_id, *upstream_subscribe_id));
                }
            }
        }

        None
    }

    // TODO: Define the behavior if the last subscriber unsubscribes from the track
    // fn delete_relation
}

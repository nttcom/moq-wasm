use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::{CacheObject, ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{bail, Ok, Result};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{object_status::ObjectStatus, DataStreams},
    },
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};
use wtransport::Connection;

struct ObjectCacheKey {
    session_id: usize,
    subscribe_id: u64,
}

impl ObjectCacheKey {
    fn new(session_id: usize, subscribe_id: u64) -> Self {
        ObjectCacheKey {
            session_id,
            subscribe_id,
        }
    }

    fn session_id(&self) -> usize {
        self.session_id
    }

    fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}

pub(crate) struct DatagramForwarder {
    session: Arc<Connection>,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_subscription: Subscription,
    object_cache_key: ObjectCacheKey,
    sleep_time: Duration,
}

impl DatagramForwarder {
    pub(crate) async fn init(
        session: Arc<Connection>,
        downstream_subscribe_id: u64,
        client: Arc<Mutex<MOQTClient>>,
    ) -> Result<Self> {
        let senders = client.lock().await.senders();
        let sleep_time = Duration::from_millis(10);
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

        let downstream_session_id = session.stable_id();

        let downstream_subscription = pubsub_relation_manager
            .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        // Get the information of the original publisher who has the track being requested
        let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
            .get_related_publisher(downstream_session_id, downstream_subscribe_id)
            .await?;

        let object_cache_key = ObjectCacheKey::new(upstream_session_id, upstream_subscribe_id);

        let datagram_forwarder = DatagramForwarder {
            session,
            senders,
            downstream_subscribe_id,
            downstream_subscription,
            object_cache_key,
            sleep_time,
        };

        Ok(datagram_forwarder)
    }

    pub(crate) async fn start(&mut self) -> Result<()> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let upstream_forwarding_preference = self.get_upstream_forwarding_preference().await?;
        self.validate_forwarding_preference(&upstream_forwarding_preference)
            .await?;
        self.set_forwarding_preference_to_downstream(upstream_forwarding_preference)
            .await?;

        self.forward_objects(&mut object_cache_storage).await?;

        Ok(())
    }

    pub(crate) async fn finish(&self) -> Result<()> {
        let downstream_session_id = self.session.stable_id();
        let downstream_stream_id = 0; // stream_id of datagram does not exist (TODO: delete buffer manager)
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: downstream_session_id,
                stream_id: downstream_stream_id,
            })
            .await?;

        tracing::info!("DatagramForwarder finished");

        Ok(())
    }

    async fn get_upstream_forwarding_preference(&self) -> Result<Option<ForwardingPreference>> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        let upstream_session_id = self.object_cache_key.session_id();
        let upstream_subscribe_id = self.object_cache_key.subscribe_id();

        pubsub_relation_manager
            .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
            .await
    }

    async fn validate_forwarding_preference(
        &self,
        upstream_forwarding_preference: &Option<ForwardingPreference>,
    ) -> Result<()> {
        match upstream_forwarding_preference {
            Some(ForwardingPreference::Datagram) => Ok(()),
            _ => {
                let msg = "Forwarding preference is not Datagram";
                bail!(msg)
            }
        }
    }

    async fn set_forwarding_preference_to_downstream(
        &self,
        upstream_forwarding_preference: Option<ForwardingPreference>,
    ) -> Result<()> {
        let forwarding_preference = upstream_forwarding_preference.unwrap();
        let downstream_session_id = self.session.stable_id();
        let downstream_subscribe_id = self.downstream_subscribe_id;

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        pubsub_relation_manager
            .set_downstream_forwarding_preference(
                downstream_session_id,
                downstream_subscribe_id,
                forwarding_preference,
            )
            .await?;

        Ok(())
    }

    async fn forward_objects(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<()> {
        let mut object_cache_id = None;
        let mut is_end = false;

        loop {
            if is_end {
                break;
            }

            (object_cache_id, is_end) = self
                .forward_object(object_cache_storage, object_cache_id)
                .await?;
        }

        Ok(())
    }

    async fn forward_object(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<(Option<usize>, bool)> {
        // Do loop until get an object from the cache storage
        loop {
            let cache = self
                .try_to_get_object(object_cache_storage, cache_id)
                .await?;

            if cache.is_none() {
                // If there is no object in the cache storage, sleep for a while and try again
                thread::sleep(self.sleep_time);
                continue;
            }

            let (cache_id, cache_object) = cache.unwrap();

            let message_buf = self.packetize(&cache_object).await?;
            self.send(message_buf).await?;

            let is_end = self.judge_end_of_forwarding(&cache_object).await?;

            return Ok((Some(cache_id), is_end));
        }
    }

    async fn try_to_get_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<Option<(usize, CacheObject)>> {
        let cache = match cache_id {
            // Try to get the first object according to Filter Type
            None => self.try_to_get_first_object(object_cache_storage).await?,
            Some(cache_id) => {
                // Try to get the subsequent object with cache_id
                self.try_to_get_subsequent_object(object_cache_storage, cache_id)
                    .await?
            }
        };

        Ok(cache)
    }

    async fn try_to_get_first_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, CacheObject)>> {
        let filter_type = self.downstream_subscription.get_filter_type();
        let upstream_session_id = self.object_cache_key.session_id();
        let upstream_subscribe_id = self.object_cache_key.subscribe_id();

        match filter_type {
            FilterType::LatestGroup => {
                object_cache_storage
                    .get_latest_group(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::LatestObject => {
                object_cache_storage
                    .get_latest_object(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                let (start_group, start_object) = self.downstream_subscription.get_absolute_start();

                object_cache_storage
                    .get_absolute_object(
                        upstream_session_id,
                        upstream_subscribe_id,
                        start_group.unwrap(),
                        start_object.unwrap(),
                    )
                    .await
            }
        }
    }

    async fn try_to_get_subsequent_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        object_cache_id: usize,
    ) -> Result<Option<(usize, CacheObject)>> {
        let upstream_session_id = self.object_cache_key.session_id();
        let upstream_subscribe_id = self.object_cache_key.subscribe_id();

        object_cache_storage
            .get_next_object(upstream_session_id, upstream_subscribe_id, object_cache_id)
            .await
    }

    async fn packetize(&mut self, cache_object: &CacheObject) -> Result<BytesMut> {
        let object_datagram = match cache_object {
            CacheObject::Datagram(d) => d,
            _ => bail!("cache object not matched"),
        };

        let mut buf = BytesMut::new();
        object_datagram.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::ObjectDatagram) as u64,
        ));
        message_buf.extend(buf);

        Ok(message_buf)
    }

    async fn send(&mut self, message_buf: BytesMut) -> Result<()> {
        if let Err(e) = self.session.send_datagram(&message_buf) {
            tracing::warn!("Failed to send datagram: {:?}", e);
            bail!(e);
        }

        Ok(())
    }

    async fn judge_end_of_forwarding(&self, cache_object: &CacheObject) -> Result<bool> {
        let is_end_of_data_stream = self.judge_end_of_data_stream(cache_object).await?;
        if is_end_of_data_stream {
            return Ok(true);
        }

        let filter_type = self.downstream_subscription.get_filter_type();
        if filter_type == FilterType::AbsoluteRange {
            let is_end_of_absolute_range = self.judge_end_of_absolute_range(cache_object).await?;
            if is_end_of_absolute_range {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn judge_end_of_data_stream(&self, cache_object: &CacheObject) -> Result<bool> {
        let is_end = match cache_object {
            CacheObject::Datagram(object_datagram) => {
                matches!(
                    object_datagram.object_status(),
                    Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
            _ => {
                let msg = "cache object not matched";
                bail!(msg)
            }
        };

        Ok(is_end)
    }

    async fn judge_end_of_absolute_range(&self, cache_object: &CacheObject) -> Result<bool> {
        let (end_group, end_object) = self.downstream_subscription.get_absolute_end();
        let end_group = end_group.unwrap();
        let end_object = end_object.unwrap();

        let is_end = match cache_object {
            CacheObject::Datagram(object_datagram) => {
                let is_group_end = object_datagram.group_id() == end_group;
                let is_object_end = object_datagram.object_id() == end_object;
                let is_ending = is_group_end && is_object_end;

                let is_ended = object_datagram.group_id() > end_group;

                is_ending || is_ended
            }
            _ => {
                let msg = "cache object not matched";
                bail!(msg)
            }
        };

        Ok(is_end)
    }
}

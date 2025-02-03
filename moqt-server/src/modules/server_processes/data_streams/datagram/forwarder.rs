use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::{self, CacheKey, ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{bail, Ok, Result};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{datagram, object_status::ObjectStatus, DataStreams},
    },
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};
use wtransport::Connection;

pub(crate) struct DatagramObjectForwarder {
    session: Arc<Connection>,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_subscription: Subscription,
    cache_key: CacheKey,
    sleep_time: Duration,
}

impl DatagramObjectForwarder {
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

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);

        let datagram_object_forwarder = DatagramObjectForwarder {
            session,
            senders,
            downstream_subscribe_id,
            downstream_subscription,
            cache_key,
            sleep_time,
        };

        Ok(datagram_object_forwarder)
    }

    pub(crate) async fn start(&mut self) -> Result<()> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let upstream_forwarding_preference = self.get_upstream_forwarding_preference().await?;
        self.validate_forwarding_preference(&upstream_forwarding_preference)
            .await?;

        let downstream_forwarding_preference = upstream_forwarding_preference.clone();
        self.set_forwarding_preference(downstream_forwarding_preference)
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

        tracing::info!("DatagramObjectForwarder finished");

        Ok(())
    }

    async fn get_upstream_forwarding_preference(&self) -> Result<Option<ForwardingPreference>> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        let upstream_session_id = self.cache_key.session_id();
        let upstream_subscribe_id = self.cache_key.subscribe_id();

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

    async fn set_forwarding_preference(
        &self,
        downstream_forwarding_preference: Option<ForwardingPreference>,
    ) -> Result<()> {
        let forwarding_preference = downstream_forwarding_preference.unwrap();
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

        while !is_end {
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
            let (cache_id, datagram_object) =
                match self.try_get_object(object_cache_storage, cache_id).await? {
                    Some((id, object)) => (id, object),
                    None => {
                        // If there is no object in the cache storage, sleep for a while and try again
                        thread::sleep(self.sleep_time);
                        continue;
                    }
                };

            let message_buf = self.packetize(&datagram_object).await?;
            self.send(message_buf).await?;

            let is_end = self.is_subscription_ended(&datagram_object)
                || self.is_data_stream_ended(&datagram_object);

            return Ok((Some(cache_id), is_end));
        }
    }

    async fn try_get_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<Option<(usize, datagram::Object)>> {
        let cache = match cache_id {
            // Try to get the first object according to Filter Type
            None => self.try_get_first_object(object_cache_storage).await?,
            Some(cache_id) => {
                // Try to get the subsequent object with cache_id
                self.try_get_subsequent_object(object_cache_storage, cache_id)
                    .await?
            }
        };

        match cache {
            None => Ok(None),
            Some((cache_id, object_cache_storage::Object::Datagram(object))) => {
                Ok(Some((cache_id, object)))
            }
            _ => {
                let msg = "object cache not matched";
                bail!(msg)
            }
        }
    }

    async fn try_get_first_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, object_cache_storage::Object)>> {
        let filter_type = self.downstream_subscription.get_filter_type();

        match filter_type {
            FilterType::LatestGroup => object_cache_storage.get_latest_group(&self.cache_key).await,
            FilterType::LatestObject => {
                object_cache_storage
                    .get_latest_object(&self.cache_key)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                let (start_group, start_object) = self.downstream_subscription.get_absolute_start();

                object_cache_storage
                    .get_absolute_object(
                        &self.cache_key,
                        start_group.unwrap(),
                        start_object.unwrap(),
                    )
                    .await
            }
        }
    }

    async fn try_get_subsequent_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        object_cache_id: usize,
    ) -> Result<Option<(usize, object_cache_storage::Object)>> {
        object_cache_storage
            .get_next_object(&self.cache_key, object_cache_id)
            .await
    }

    async fn packetize(&mut self, datagram_object: &datagram::Object) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        datagram_object.packetize(&mut buf);

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

    fn is_subscription_ended(&self, datagram_object: &datagram::Object) -> bool {
        let group_id = datagram_object.group_id();
        let object_id = datagram_object.object_id();

        self.downstream_subscription.is_end(group_id, object_id)
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfSubgroup, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    fn is_data_stream_ended(&self, datagram_object: &datagram::Object) -> bool {
        matches!(
            datagram_object.object_status(),
            Some(ObjectStatus::EndOfTrackAndGroup)
        )
    }
}

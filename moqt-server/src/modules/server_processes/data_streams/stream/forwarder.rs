use super::uni_stream::UniSendStream;
use crate::{
    modules::{
        buffer_manager::BufferCommand,
        moqt_client::MOQTClient,
        object_cache_storage::{cache::CacheKey, wrapper::ObjectCacheStorageWrapper},
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    SubgroupStreamId,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{object_status::ObjectStatus, subgroup_stream, DataStreams},
    },
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};

pub(crate) struct SubgroupStreamObjectForwarder {
    stream: UniSendStream,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_subscription: Subscription,
    cache_key: CacheKey,
    subgroup_stream_id: Option<SubgroupStreamId>,
    sleep_time: Duration,
}

impl SubgroupStreamObjectForwarder {
    pub(crate) async fn init(
        stream: UniSendStream,
        downstream_subscribe_id: u64,
        client: Arc<Mutex<MOQTClient>>,
        subgroup_stream_id: Option<SubgroupStreamId>,
    ) -> Result<Self> {
        let senders = client.lock().await.senders();
        let sleep_time = Duration::from_millis(10);
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

        let downstream_session_id = stream.stable_id();

        let downstream_subscription = pubsub_relation_manager
            .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        // Get the information of the original publisher who has the track being requested
        let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
            .get_related_publisher(downstream_session_id, downstream_subscribe_id)
            .await?;

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);

        let stream_object_forwarder = SubgroupStreamObjectForwarder {
            stream,
            senders,
            downstream_subscribe_id,
            downstream_subscription,
            cache_key,
            subgroup_stream_id,
            sleep_time,
        };

        Ok(stream_object_forwarder)
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

        self.forward_header(&mut object_cache_storage).await?;

        self.forward_objects(&mut object_cache_storage).await?;

        Ok(())
    }

    pub(crate) async fn finish(&self) -> Result<()> {
        let downstream_session_id = self.stream.stable_id();
        let downstream_stream_id = self.stream.stream_id();
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: downstream_session_id,
                stream_id: downstream_stream_id,
            })
            .await?;

        tracing::info!("SubgroupStreamObjectForwarder finished");

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
            Some(ForwardingPreference::Subgroup) => Ok(()),
            _ => {
                bail!("Forwarding preference is not Subgroup Stream");
            }
        }
    }

    async fn set_forwarding_preference(
        &self,
        downstream_forwarding_preference: Option<ForwardingPreference>,
    ) -> Result<()> {
        let forwarding_preference = downstream_forwarding_preference.unwrap();
        let downstream_session_id = self.stream.stable_id();
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

    async fn forward_header(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<()> {
        let stream_header = self.get_header(object_cache_storage).await?;

        let message_buf = self.packetize_header(&stream_header).await?;

        self.send(message_buf).await?;

        Ok(())
    }

    async fn get_header(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<subgroup_stream::Header> {
        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();
        let subgroup_stream_header = object_cache_storage
            .get_subgroup_stream_header(&self.cache_key, group_id, subgroup_id)
            .await?;

        Ok(subgroup_stream_header)
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
            let (cache_id, stream_object) =
                match self.try_get_object(object_cache_storage, cache_id).await? {
                    Some((id, object)) => (id, object),
                    None => {
                        // If there is no object in the cache storage, sleep for a while and try again
                        thread::sleep(self.sleep_time);
                        continue;
                    }
                };

            let message_buf = self.packetize_object(&stream_object).await?;
            self.send(message_buf).await?;

            let is_end = self.is_subscription_ended(&stream_object)
                || self.is_data_stream_ended(&stream_object);

            return Ok((Some(cache_id), is_end));
        }
    }

    async fn try_get_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        match cache_id {
            // Try to get the first object according to Filter Type
            None => self.try_get_first_object(object_cache_storage).await,
            Some(cache_id) => {
                // Try to get the subsequent object with cache_id
                self.try_get_subsequent_object(object_cache_storage, cache_id)
                    .await
            }
        }
    }

    async fn try_get_first_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let filter_type = self.downstream_subscription.get_filter_type();
        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();

        match filter_type {
            FilterType::LatestGroup => {
                // Try to obtain the first object in the subgroup stream specified by the arguments.
                // This operation is the same on the first stream and on subsequent streams.
                object_cache_storage
                    .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                    .await
            }
            // Currently not supported
            FilterType::LatestObject => {
                // Try to obtain the first object in the subgroup stream specified by the arguments.
                // TODO: If it's on the first subgroup stream, it should get the latest object.
                //       To distinguish the first stream, we need to modify downstream subscription.
                //       (e.g. Implementation of FilterType::AbsoluteStart | FilterType::AbsoluteRange)
                object_cache_storage
                    .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                let (start_group, start_object) = self.downstream_subscription.get_absolute_start();
                let start_group = start_group.unwrap();
                let start_object = start_object.unwrap();

                if group_id == start_group {
                    object_cache_storage
                        .get_absolute_subgroup_stream_object(
                            &self.cache_key,
                            group_id,
                            subgroup_id,
                            start_object,
                        )
                        .await
                } else {
                    object_cache_storage
                        .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                        .await
                }
            }
        }
    }

    async fn try_get_subsequent_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        object_cache_id: usize,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();
        object_cache_storage
            .get_next_subgroup_stream_object(
                &self.cache_key,
                group_id,
                subgroup_id,
                object_cache_id,
            )
            .await
    }

    async fn packetize_header(&self, header: &subgroup_stream::Header) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        let downstream_subscribe_id = self.downstream_subscribe_id;
        let downstream_track_alias = self.downstream_subscription.get_track_alias();

        let header = subgroup_stream::Header::new(
            downstream_subscribe_id,
            downstream_track_alias,
            header.group_id(),
            header.subgroup_id(),
            header.publisher_priority(),
        )
        .unwrap();

        header.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::StreamHeaderSubgroup) as u64,
        ));
        message_buf.extend(buf);

        Ok(message_buf)
    }

    async fn packetize_object(
        &mut self,
        stream_object: &subgroup_stream::Object,
    ) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        stream_object.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(buf);

        Ok(message_buf)
    }

    async fn send(&mut self, message_buf: BytesMut) -> Result<()> {
        if let Err(e) = self.stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            bail!(e);
        }

        Ok(())
    }

    fn is_subscription_ended(&self, stream_object: &subgroup_stream::Object) -> bool {
        let group_id = self.subgroup_stream_id.unwrap().0;
        let object_id = stream_object.object_id();

        self.downstream_subscription.is_end(group_id, object_id)
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfSubgroup, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    fn is_data_stream_ended(&self, stream_object: &subgroup_stream::Object) -> bool {
        matches!(
            stream_object.object_status(),
            Some(ObjectStatus::EndOfSubgroup)
                | Some(ObjectStatus::EndOfGroup)
                | Some(ObjectStatus::EndOfTrackAndGroup)
        )
    }
}

use super::uni_stream::UniSendStream;
use crate::modules::{
    buffer_manager::BufferCommand,
    message_handlers::{stream_header::StreamHeader, stream_object::StreamObject},
    moqt_client::MOQTClient,
    object_cache_storage::{self, CacheKey, ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{object_status::ObjectStatus, subgroup_stream, track_stream, DataStreams},
    },
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};

pub(crate) struct StreamObjectForwarder {
    stream: UniSendStream,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_subscription: Subscription,
    data_stream_type: DataStreamType,
    cache_key: CacheKey,
    sleep_time: Duration,
}

impl StreamObjectForwarder {
    pub(crate) async fn init(
        stream: UniSendStream,
        downstream_subscribe_id: u64,
        client: Arc<Mutex<MOQTClient>>,
        data_stream_type: DataStreamType,
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

        let stream_object_forwarder = StreamObjectForwarder {
            stream,
            senders,
            downstream_subscribe_id,
            downstream_subscription,
            data_stream_type,
            cache_key,
            sleep_time,
        };

        Ok(stream_object_forwarder)
    }

    pub(crate) async fn start(&mut self) -> Result<()> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut subgroup_group_id: Option<u64> = None;

        let upstream_forwarding_preference = self.get_upstream_forwarding_preference().await?;
        self.validate_forwarding_preference(&upstream_forwarding_preference)
            .await?;

        let downstream_forwarding_preference = upstream_forwarding_preference.clone();
        self.set_forwarding_preference(downstream_forwarding_preference)
            .await?;

        self.forward_header(&mut object_cache_storage, &mut subgroup_group_id)
            .await?;

        self.forward_objects(&mut object_cache_storage, &subgroup_group_id)
            .await?;

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

        tracing::info!("StreamObjectForwarder finished");

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
            Some(ForwardingPreference::Track) => self.check_data_stream_type_track().await?,
            Some(ForwardingPreference::Subgroup) => self.check_data_stream_type_subgroup().await?,
            _ => {
                bail!("Forwarding preference is not Stream");
            }
        }

        Ok(())
    }

    async fn check_data_stream_type_track(&self) -> Result<()> {
        if self.data_stream_type != DataStreamType::StreamHeaderTrack {
            bail!(
                "uni send stream's data stream type is wrong (expected Track, but got {:?})",
                self.data_stream_type
            );
        }

        Ok(())
    }

    async fn check_data_stream_type_subgroup(&self) -> Result<()> {
        if self.data_stream_type != DataStreamType::StreamHeaderSubgroup {
            bail!(
                "uni send stream's data stream type is wrong (expected Subgroup, but got {:?})",
                self.data_stream_type
            );
        }

        Ok(())
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
        subgroup_group_id: &mut Option<u64>,
    ) -> Result<()> {
        let stream_header = self.get_header(object_cache_storage).await?;

        let message_buf = self
            .packetize_header(&stream_header, subgroup_group_id)
            .await?;

        self.send(message_buf).await?;

        Ok(())
    }

    async fn get_header(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<StreamHeader> {
        let header_cache = object_cache_storage.get_header(&self.cache_key).await;

        match header_cache {
            Ok(object_cache_storage::Header::Track(header)) => Ok(StreamHeader::Track(header)),
            Ok(object_cache_storage::Header::Subgroup(header)) => {
                Ok(StreamHeader::Subgroup(header))
            }
            _ => {
                let msg = "header cache not matched";
                bail!(msg)
            }
        }
    }

    async fn forward_objects(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        subgroup_group_id: &Option<u64>,
    ) -> Result<()> {
        let mut object_cache_id = None;
        let mut is_end = false;

        while !is_end {
            (object_cache_id, is_end) = self
                .forward_object(object_cache_storage, object_cache_id, subgroup_group_id)
                .await?;
        }

        Ok(())
    }

    async fn forward_object(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
        subgroup_group_id: &Option<u64>,
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

            let is_end = self.is_subscription_ended(&stream_object, subgroup_group_id)
                || self.is_data_stream_ended(&stream_object);

            return Ok((Some(cache_id), is_end));
        }
    }

    async fn try_get_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<Option<(usize, StreamObject)>> {
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
            Some((cache_id, object_cache_storage::Object::Track(object))) => {
                let object = StreamObject::Track(object);
                Ok(Some((cache_id, object)))
            }
            Some((cache_id, object_cache_storage::Object::Subgroup(object))) => {
                let object = StreamObject::Subgroup(object);
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

    async fn packetize_header(
        &mut self,
        stream_header: &StreamHeader,
        subgroup_group_id: &mut Option<u64>,
    ) -> Result<BytesMut> {
        let message_buf = match stream_header {
            StreamHeader::Track(header) => self.packetize_track_header(header),
            StreamHeader::Subgroup(header) => {
                self.packetize_subgroup_header(header, subgroup_group_id)
            }
        };

        Ok(message_buf)
    }

    fn packetize_track_header(&self, header: &track_stream::Header) -> BytesMut {
        let mut buf = BytesMut::new();
        let downstream_subscribe_id = self.downstream_subscribe_id;
        let downstream_track_alias = self.downstream_subscription.get_track_alias();

        let header = track_stream::Header::new(
            downstream_subscribe_id,
            downstream_track_alias,
            header.publisher_priority(),
        )
        .unwrap();

        header.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::StreamHeaderTrack) as u64,
        ));
        message_buf.extend(buf);

        message_buf
    }

    fn packetize_subgroup_header(
        &self,
        header: &subgroup_stream::Header,
        subgroup_group_id: &mut Option<u64>,
    ) -> BytesMut {
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

        *subgroup_group_id = Some(header.group_id());

        header.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::StreamHeaderSubgroup) as u64,
        ));
        message_buf.extend(buf);

        message_buf
    }

    async fn packetize_object(&mut self, stream_object: &StreamObject) -> Result<BytesMut> {
        let mut buf = BytesMut::new();

        match stream_object {
            StreamObject::Track(track_object) => track_object.packetize(&mut buf),
            StreamObject::Subgroup(subgroup_object) => subgroup_object.packetize(&mut buf),
        }

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

    fn is_subscription_ended(
        &self,
        stream_object: &StreamObject,
        subgroup_group_id: &Option<u64>,
    ) -> bool {
        let (group_id, object_id) = match stream_object {
            StreamObject::Track(track_stream_object) => (
                track_stream_object.group_id(),
                track_stream_object.object_id(),
            ),
            StreamObject::Subgroup(subgroup_stream_object) => (
                subgroup_group_id.unwrap(),
                subgroup_stream_object.object_id(),
            ),
        };

        self.downstream_subscription.is_end(group_id, object_id)
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfSubgroup, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    fn is_data_stream_ended(&self, stream_object: &StreamObject) -> bool {
        match stream_object {
            StreamObject::Track(track_stream_object) => {
                matches!(
                    track_stream_object.object_status(),
                    Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
            StreamObject::Subgroup(subgroup_stream_object) => {
                matches!(
                    subgroup_stream_object.object_status(),
                    Some(ObjectStatus::EndOfSubgroup)
                        | Some(ObjectStatus::EndOfGroup)
                        | Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
        }
    }
}

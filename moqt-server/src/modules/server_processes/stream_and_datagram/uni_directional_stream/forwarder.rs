use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::{CacheHeader, CacheObject, ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{
            object_status::ObjectStatus, object_stream_subgroup::ObjectStreamSubgroup,
            object_stream_track::ObjectStreamTrack, stream_header_subgroup::StreamHeaderSubgroup,
            stream_header_track::StreamHeaderTrack, DataStreams,
        },
    },
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, thread, time::Duration};
use tokio::sync::Mutex;
use tracing::{self};

use super::streams::UniSendStream;

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

pub(crate) struct ObjectStreamForwarder {
    stream: UniSendStream,
    senders: Arc<Senders>,
    downstream_subscription: Subscription,
    data_stream_type: DataStreamType,
    object_cache_key: ObjectCacheKey,
    sleep_time: Duration,
}

impl ObjectStreamForwarder {
    pub(crate) async fn init(
        stream: UniSendStream,
        client: Arc<Mutex<MOQTClient>>,
        data_stream_type: DataStreamType,
    ) -> Result<Self> {
        let senders = client.lock().await.senders();
        let sleep_time = Duration::from_millis(10);
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

        let downstream_session_id = stream.stable_id();
        let downstream_subscribe_id = stream.subscribe_id();

        let downstream_subscription = pubsub_relation_manager
            .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        // Get the information of the original publisher who has the track being requested
        let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
            .get_related_publisher(downstream_session_id, downstream_subscribe_id)
            .await?;

        let object_cache_key = ObjectCacheKey::new(upstream_session_id, upstream_subscribe_id);

        let object_stream_forwarder = ObjectStreamForwarder {
            stream,
            senders,
            downstream_subscription,
            data_stream_type,
            object_cache_key,
            sleep_time,
        };

        Ok(object_stream_forwarder)
    }

    pub(crate) async fn start(&mut self) -> Result<()> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut subgroup_group_id: Option<u64> = None;

        self.check_and_set_forwarding_preference().await?;

        self.get_and_forward_header(&mut object_cache_storage, &mut subgroup_group_id)
            .await?;

        self.forward_loop(&mut object_cache_storage, &subgroup_group_id)
            .await?;

        Ok(())
    }

    pub(crate) async fn terminate(&self, code: TerminationErrorCode, reason: String) -> Result<()> {
        self.senders
            .close_session_tx()
            .send((u8::from(code) as u64, reason.to_string()))
            .await?;

        let downstream_session_id = self.stream.stable_id();
        let downstream_stream_id = self.stream.stream_id();
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: downstream_session_id,
                stream_id: downstream_stream_id,
            })
            .await?;

        Ok(())
    }

    async fn check_and_set_forwarding_preference(&self) -> Result<()> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        let downstream_session_id = self.stream.stable_id();
        let downstream_subscribe_id = self.stream.subscribe_id();
        let upstream_session_id = self.object_cache_key.session_id();
        let upstream_subscribe_id = self.object_cache_key.subscribe_id();

        match pubsub_relation_manager
            .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
            .await?
        {
            Some(ForwardingPreference::Track) => {
                if self.data_stream_type == DataStreamType::StreamHeaderTrack {
                    pubsub_relation_manager
                        .set_downstream_forwarding_preference(
                            downstream_session_id,
                            downstream_subscribe_id,
                            ForwardingPreference::Track,
                        )
                        .await?;
                } else {
                    let msg = std::format!(
        "uni send stream's data stream type is wrong (expected Track, but got {:?})",
        self.data_stream_type
    );
                    bail!(msg);
                }
            }
            Some(ForwardingPreference::Subgroup) => {
                if self.data_stream_type == DataStreamType::StreamHeaderSubgroup {
                    pubsub_relation_manager
                        .set_downstream_forwarding_preference(
                            downstream_session_id,
                            downstream_subscribe_id,
                            ForwardingPreference::Subgroup,
                        )
                        .await?;
                } else {
                    let msg = std::format!(
        "uni send stream's data stream type is wrong (expected Subgroup, but got {:?})",
        self.data_stream_type
    );
                    bail!(msg);
                }
            }

            _ => {
                let msg = "Invalid forwarding preference";
                bail!(msg);
            }
        };

        Ok(())
    }

    async fn get_and_forward_header(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        subgroup_group_id: &mut Option<u64>,
    ) -> Result<()> {
        let upstream_session_id = self.object_cache_key.session_id();
        let upstream_subscribe_id = self.object_cache_key.subscribe_id();

        // Get the header from the cache storage and send it to the client
        let message_buf = match object_cache_storage
            .get_header(upstream_session_id, upstream_subscribe_id)
            .await?
        {
            CacheHeader::Track(stream_header_track) => {
                self.packetize_forwarding_stream_header_track(&stream_header_track)
                    .await
            }
            CacheHeader::Subgroup(stream_header_subgroup) => {
                self.packetize_forwarding_stream_header_subgroup(
                    &stream_header_subgroup,
                    subgroup_group_id,
                )
                .await
            }
            _ => {
                let msg = "cache header not matched";
                bail!(msg)
            }
        };

        if let Err(e) = self.stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            bail!(e);
        }

        Ok(())
    }

    async fn forward_loop(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        subgroup_group_id: &Option<u64>,
    ) -> Result<()> {
        let mut object_cache_id = None;
        let mut is_end = false;

        loop {
            if is_end {
                break;
            }

            (object_cache_id, is_end) = self
                .get_and_forward_object(object_cache_storage, object_cache_id, subgroup_group_id)
                .await?;
        }

        Ok(())
    }

    async fn get_and_forward_object(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
        subgroup_group_id: &Option<u64>,
    ) -> Result<(Option<usize>, bool)> {
        // Do loop until get an object from the cache storage
        loop {
            let cache = match cache_id {
                None => self.try_to_get_first_object(object_cache_storage).await?,
                Some(cache_id) => {
                    self.try_to_get_subsequent_object(object_cache_storage, cache_id)
                        .await?
                }
            };

            match cache {
                None => {
                    // If there is no object in the cache storage, sleep for a while and try again
                    thread::sleep(self.sleep_time);
                    continue;
                }
                Some((cache_id, cache_object)) => {
                    self.packetize_and_forward_object(&cache_object).await?;

                    let is_end = self
                        .judge_end_of_forwarding(&cache_object, subgroup_group_id)
                        .await?;

                    return Ok((Some(cache_id), is_end));
                }
            }
        }
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

    async fn packetize_and_forward_object(&mut self, cache_object: &CacheObject) -> Result<()> {
        let message_buf = match cache_object {
            CacheObject::Track(object_stream_track) => {
                self.packetize_forwarding_object_stream_track(object_stream_track)
                    .await
            }
            CacheObject::Subgroup(object_stream_subgroup) => {
                self.packetize_forwarding_object_stream_subgroup(object_stream_subgroup)
                    .await
            }
            _ => {
                let msg = "cache object not matched";
                bail!(msg)
            }
        };

        if let Err(e) = self.stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            bail!(e);
        }

        Ok(())
    }

    async fn judge_end_of_forwarding(
        &self,
        cache_object: &CacheObject,
        subgroup_group_id: &Option<u64>,
    ) -> Result<bool> {
        let is_end_of_data_stream = self.judge_end_of_data_stream(cache_object).await?;
        if is_end_of_data_stream {
            return Ok(true);
        }

        let filter_type = self.downstream_subscription.get_filter_type();
        if filter_type == FilterType::AbsoluteRange {
            let is_end_of_absolute_range = self
                .judge_end_of_absolute_range(cache_object, subgroup_group_id)
                .await?;
            if is_end_of_absolute_range {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn judge_end_of_data_stream(&self, cache_object: &CacheObject) -> Result<bool> {
        let is_end = match cache_object {
            CacheObject::Track(object_stream_track) => {
                matches!(
                    object_stream_track.object_status(),
                    Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
            CacheObject::Subgroup(object_stream_subgroup) => {
                matches!(
                    object_stream_subgroup.object_status(),
                    Some(ObjectStatus::EndOfSubgroup)
                        | Some(ObjectStatus::EndOfGroup)
                        | Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
            _ => {
                let msg = "cache object not matched";
                bail!(msg)
            }
        };

        Ok(is_end)
    }

    async fn judge_end_of_absolute_range(
        &self,
        cache_object: &CacheObject,
        subgroup_group_id: &Option<u64>,
    ) -> Result<bool> {
        let (end_group, end_object) = self.downstream_subscription.get_absolute_end();
        let end_group = end_group.unwrap();
        let end_object = end_object.unwrap();

        let is_end = match cache_object {
            CacheObject::Track(object_stream_track) => {
                let is_group_end = object_stream_track.group_id() == end_group;
                let is_object_end = object_stream_track.object_id() == end_object;
                let is_ending = is_group_end && is_object_end;

                let is_ended = object_stream_track.group_id() > end_group;

                is_ending || is_ended
            }
            CacheObject::Subgroup(object_stream_subgroup) => {
                let subgroup_group_id = subgroup_group_id.unwrap();
                let is_group_end = subgroup_group_id == end_group;
                let is_object_end = object_stream_subgroup.object_id() == end_object;
                let is_ending = is_group_end && is_object_end;

                let is_ended = subgroup_group_id > end_group;

                is_ending || is_ended
            }
            _ => {
                let msg = "cache object not matched";
                bail!(msg)
            }
        };

        Ok(is_end)
    }

    async fn packetize_forwarding_stream_header_track(
        &self,
        header: &StreamHeaderTrack,
    ) -> BytesMut {
        let mut buf = BytesMut::new();
        let downstream_subscribe_id = self.stream.subscribe_id();
        let downstream_track_alias = self.downstream_subscription.get_track_alias();

        let header = StreamHeaderTrack::new(
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

    async fn packetize_forwarding_stream_header_subgroup(
        &self,
        header: &StreamHeaderSubgroup,
        subgroup_group_id: &mut Option<u64>,
    ) -> BytesMut {
        let mut buf = BytesMut::new();
        let downstream_subscribe_id = self.stream.subscribe_id();
        let downstream_track_alias = self.downstream_subscription.get_track_alias();

        let header = StreamHeaderSubgroup::new(
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

    async fn packetize_forwarding_object_stream_track(
        &self,
        object: &ObjectStreamTrack,
    ) -> BytesMut {
        let mut buf = BytesMut::new();
        object.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(buf);

        message_buf
    }

    async fn packetize_forwarding_object_stream_subgroup(
        &self,
        object: &ObjectStreamSubgroup,
    ) -> BytesMut {
        let mut buf = BytesMut::new();
        object.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(buf);

        message_buf
    }
}

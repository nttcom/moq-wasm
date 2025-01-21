use self::{
    object_cache_storage::CacheKey, object_stream::StreamObject, stream_header::StreamHeader,
};

use super::uni_stream::UniRecvStream;
use crate::{
    modules::{
        buffer_manager::{request_buffer, BufferCommand},
        message_handlers::{
            object_stream::{self, ObjectStreamProcessResult},
            stream_header::{self, StreamHeaderProcessResult},
        },
        moqt_client::MOQTClient,
        object_cache_storage::{self, ObjectCacheStorageWrapper},
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    TerminationError,
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode,
    data_stream_type::DataStreamType,
    messages::data_streams::object_status::ObjectStatus,
    models::{subscriptions::Subscription, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self};

pub(crate) struct ObjectStreamReceiver {
    stream: UniRecvStream,
    buf: Arc<Mutex<BytesMut>>,
    senders: Arc<Senders>,
    client: Arc<Mutex<MOQTClient>>,
    duration: u64,
    subscribe_id: Option<u64>,
    data_stream_type: Option<DataStreamType>,
    upstream_subscription: Option<Subscription>,
}

impl ObjectStreamReceiver {
    pub(crate) async fn init(stream: UniRecvStream, client: Arc<Mutex<MOQTClient>>) -> Self {
        let senders = client.lock().await.senders();
        let stable_id = stream.stable_id();
        let stream_id = stream.stream_id();
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;
        // TODO: Set the accurate duration
        let duration = 100000;

        ObjectStreamReceiver {
            stream,
            buf,
            senders,
            client,
            duration,
            subscribe_id: None,
            data_stream_type: None,
            upstream_subscription: None,
        }
    }

    pub(crate) async fn start(&mut self) -> Result<(), TerminationError> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut is_end = false;
        let session_id = self.client.lock().await.id();

        // If the received object is subgroup, store group id to judge the end of range.
        let mut subgroup_group_id: Option<u64> = None;

        while !is_end {
            let read_bytes = self.read_stream().await?;
            self.add_to_buf(read_bytes).await;

            if !self.has_header() {
                self.receive_header(
                    session_id,
                    &mut subgroup_group_id,
                    &mut object_cache_storage,
                )
                .await?;

                // If the header has not been received, continue to receive the header.
                if !self.has_header() {
                    continue;
                }
            }

            is_end = self
                .receive_objects(subgroup_group_id, &mut object_cache_storage)
                .await?;
        }

        Ok(())
    }

    pub(crate) async fn finish(&self) -> Result<()> {
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: self.stream.stable_id(),
                stream_id: self.stream.stream_id(),
            })
            .await?;

        tracing::debug!("ObjectStreamReceiver finished");

        Ok(())
    }

    async fn read_stream(&mut self) -> Result<BytesMut, TerminationError> {
        let mut buffer = vec![0; 65536].into_boxed_slice();

        let length: usize = match self.stream.read(&mut buffer).await {
            Ok(byte_read) => byte_read.unwrap(),
            Err(err) => {
                let msg = format!("Failed to read from stream: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        Ok(BytesMut::from(&buffer[..length]))
    }

    async fn add_to_buf(&mut self, read_buf: BytesMut) {
        let mut buf = self.buf.lock().await;
        buf.extend_from_slice(&read_buf);
    }

    fn has_header(&self) -> bool {
        self.upstream_subscription.is_some()
    }

    async fn receive_header(
        &mut self,
        session_id: usize,
        subgroup_group_id: &mut Option<u64>,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let header = match self.read_header_from_buf().await? {
            Some(header) => header,
            None => {
                return Ok(());
            }
        };

        self.set_subscribe_id(&header).await?;
        self.set_data_stream_type(&header).await?;

        let subscribe_id = self.subscribe_id.unwrap();
        let data_stream_type = self.data_stream_type.unwrap();

        self.set_upstream_forwarding_preference(session_id, subscribe_id, data_stream_type)
            .await?;
        self.set_upstream_subscription(session_id, subscribe_id)
            .await?;

        if let StreamHeader::Subgroup(header) = &header {
            *subgroup_group_id = Some(header.group_id());
        }

        // TODO: split function to create cache and store header
        self.create_cache_storage(session_id, subscribe_id, header, object_cache_storage)
            .await?;

        self.create_forwarders(session_id, subscribe_id).await?;

        Ok(())
    }

    async fn read_header_from_buf(&self) -> Result<Option<StreamHeader>, TerminationError> {
        let result = self.try_read_header_from_buf().await;

        match result {
            StreamHeaderProcessResult::Success(stream_header) => Ok(Some(stream_header)),
            StreamHeaderProcessResult::Continue => Ok(None),
            StreamHeaderProcessResult::Failure(code, reason) => {
                let msg = std::format!("stream_header_read failure: {:?}", reason);
                Err((code, msg))
            }
        }
    }

    async fn try_read_header_from_buf(&self) -> StreamHeaderProcessResult {
        let mut process_buf = self.buf.lock().await;
        let client = self.client.clone();

        stream_header::try_read_header(&mut process_buf, client).await
    }

    async fn set_subscribe_id(
        &mut self,
        stream_header: &StreamHeader,
    ) -> Result<(), TerminationError> {
        let subscribe_id = match stream_header {
            StreamHeader::Track(header) => header.subscribe_id(),
            StreamHeader::Subgroup(header) => header.subscribe_id(),
        };

        self.subscribe_id = Some(subscribe_id);

        Ok(())
    }

    async fn set_data_stream_type(
        &mut self,
        stream_header: &StreamHeader,
    ) -> Result<(), TerminationError> {
        let data_stream_type = match stream_header {
            StreamHeader::Track(_) => DataStreamType::StreamHeaderTrack,
            StreamHeader::Subgroup(_) => DataStreamType::StreamHeaderSubgroup,
        };

        self.data_stream_type = Some(data_stream_type);

        Ok(())
    }

    async fn set_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        data_stream_type: DataStreamType,
    ) -> Result<(), TerminationError> {
        let forwarding_preference = match data_stream_type {
            DataStreamType::StreamHeaderTrack => ForwardingPreference::Track,
            DataStreamType::StreamHeaderSubgroup => ForwardingPreference::Subgroup,
            _ => {
                let msg = "data_stream_type not matched".to_string();
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .set_upstream_forwarding_preference(
                upstream_session_id,
                upstream_subscribe_id,
                forwarding_preference,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to set upstream forwarding preference: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn set_upstream_subscription(
        &mut self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<(), TerminationError> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let upstream_subscription = match pubsub_relation_manager
            .get_upstream_subscription_by_ids(upstream_session_id, upstream_subscribe_id)
            .await
        {
            Ok(upstream_subscription) => upstream_subscription,
            Err(err) => {
                let msg = format!("Fail to get upstream subscription: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        if upstream_subscription.is_none() {
            let msg = "Upstream subscription not found".to_string();
            let code = TerminationErrorCode::InternalError;

            return Err((code, msg));
        }

        self.upstream_subscription = upstream_subscription;

        Ok(())
    }

    async fn create_cache_storage(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        stream_header: StreamHeader,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let header_cache = match stream_header {
            StreamHeader::Track(track_header) => object_cache_storage::Header::Track(track_header),
            StreamHeader::Subgroup(subgroup_header) => {
                object_cache_storage::Header::Subgroup(subgroup_header)
            }
        };

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        match object_cache_storage
            .set_subscription(&cache_key, header_cache)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to create cache storage: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn create_forwarders(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<(), TerminationError> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        let subscribers = match pubsub_relation_manager
            .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
            .await
        {
            Ok(subscribers) => subscribers,
            Err(err) => {
                let msg = format!("Fail to get related subscribers: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        for (downstream_session_id, downstream_subscribe_id) in subscribers {
            match self
                .create_forwarder(downstream_session_id, downstream_subscribe_id)
                .await
            {
                Ok(_) => {}
                Err(err) => {
                    let msg = format!("Fail to create forwarder: {:?}", err);
                    let code = TerminationErrorCode::InternalError;

                    return Err((code, msg));
                }
            }
        }
        Ok(())
    }

    async fn create_forwarder(
        &self,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
    ) -> Result<()> {
        let open_subscription_txes = self.senders.open_downstream_stream_or_datagram_txes();
        let data_stream_type = self.data_stream_type.unwrap();

        let open_subscription_tx = open_subscription_txes
            .lock()
            .await
            .get(&downstream_session_id)
            .unwrap()
            .clone();

        open_subscription_tx
            .send((downstream_subscribe_id, data_stream_type))
            .await?;

        Ok(())
    }

    async fn receive_objects(
        &self,
        subgroup_group_id: Option<u64>,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<bool, TerminationError> {
        let session_id = self.client.lock().await.id();
        let subscribe_id = self.subscribe_id.unwrap();
        let mut is_end = false;

        while !is_end {
            is_end = match self
                .receive_object(
                    session_id,
                    subscribe_id,
                    subgroup_group_id,
                    object_cache_storage,
                )
                .await?
            {
                Some(is_end) => is_end,
                None => break, // Return to read stream again since there is no object in the buffer.
            };
        }

        Ok(is_end)
    }

    async fn receive_object(
        &self,
        session_id: usize,
        subscribe_id: u64,
        subgroup_group_id: Option<u64>,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<bool>, TerminationError> {
        let stream_object = match self.read_object_from_buf().await? {
            Some(object) => object,
            None => {
                return Ok(None);
            }
        };

        self.store_object(
            &stream_object,
            session_id,
            subscribe_id,
            object_cache_storage,
        )
        .await?;

        let is_end = self.is_subscription_ended(&stream_object, &subgroup_group_id)
            || self.is_data_stream_ended(&stream_object);

        Ok(Some(is_end))
    }

    async fn read_object_from_buf(&self) -> Result<Option<StreamObject>, TerminationError> {
        let result = self.try_read_object_from_buf().await;

        match result {
            ObjectStreamProcessResult::Success(stream_object) => Ok(Some(stream_object)),
            ObjectStreamProcessResult::Continue => Ok(None),
            ObjectStreamProcessResult::Failure(code, reason) => {
                let msg = std::format!("object_stream_read failure: {:?}", reason);
                Err((code, msg))
            }
        }
    }

    async fn try_read_object_from_buf(&self) -> ObjectStreamProcessResult {
        let mut buf = self.buf.lock().await;
        let data_stream_type = self.data_stream_type.unwrap();

        object_stream::try_read_object(&mut buf, data_stream_type).await
    }

    async fn store_object(
        &self,
        stream_object: &StreamObject,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let object_cache = match stream_object {
            StreamObject::Track(object) => object_cache_storage::Object::Track(object.clone()),
            StreamObject::Subgroup(object) => {
                object_cache_storage::Object::Subgroup(object.clone())
            }
        };

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        match object_cache_storage
            .set_object(&cache_key, object_cache, self.duration)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to store object to cache storage: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    fn is_subscription_ended(
        &self,
        object: &StreamObject,
        subgroup_group_id: &Option<u64>,
    ) -> bool {
        let (group_id, object_id) = match object {
            StreamObject::Track(object_stream_track) => (
                object_stream_track.group_id(),
                object_stream_track.object_id(),
            ),
            StreamObject::Subgroup(object_stream_subgroup) => (
                subgroup_group_id.unwrap(),
                object_stream_subgroup.object_id(),
            ),
        };

        self.upstream_subscription
            .as_ref()
            .unwrap()
            .is_end(group_id, object_id)
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfSubgroup, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    // TODO: Add handling for FIN message
    fn is_data_stream_ended(&self, stream_object: &StreamObject) -> bool {
        match stream_object {
            StreamObject::Track(object_stream_track) => {
                matches!(
                    object_stream_track.object_status(),
                    Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
            StreamObject::Subgroup(object_stream_subgroup) => {
                matches!(
                    object_stream_subgroup.object_status(),
                    Some(ObjectStatus::EndOfSubgroup)
                        | Some(ObjectStatus::EndOfGroup)
                        | Some(ObjectStatus::EndOfTrackAndGroup)
                )
            }
        }
    }
}

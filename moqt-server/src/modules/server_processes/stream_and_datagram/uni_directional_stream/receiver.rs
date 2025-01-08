use super::streams::UniRecvStream;
use crate::{
    modules::{
        buffer_manager::{request_buffer, BufferCommand},
        message_handlers::{
            object_stream::{object_stream_handler, ObjectStreamProcessResult},
            stream_header::{stream_header_handler, StreamHeaderProcessResult},
        },
        moqt_client::MOQTClient,
        object_cache_storage::{CacheHeader, CacheObject, ObjectCacheStorageWrapper},
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
    messages::{
        control_messages::subscribe::FilterType, data_streams::object_status::ObjectStatus,
    },
    models::subscriptions::Subscription,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self};

pub(crate) struct UniStreamReceiver {
    stream: UniRecvStream,
    buf: Arc<Mutex<BytesMut>>,
    senders: Arc<Senders>,
    client: Arc<Mutex<MOQTClient>>,
    subscribe_id: Option<u64>,
    stream_header_type: Option<DataStreamType>,
    upstream_subscription: Option<Subscription>,
}

impl UniStreamReceiver {
    pub(crate) async fn init(stream: UniRecvStream, client: Arc<Mutex<MOQTClient>>) -> Self {
        let senders = client.lock().await.senders();
        let stable_id = stream.stable_id();
        let stream_id = stream.stream_id();
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;

        UniStreamReceiver {
            stream,
            buf,
            senders,
            client,
            subscribe_id: None,
            stream_header_type: None,
            upstream_subscription: None,
        }
    }

    pub(crate) async fn start(&mut self) -> Result<(), TerminationError> {
        self.receive_loop().await?;

        Ok(())
    }

    pub(crate) async fn terminate(&self) -> Result<()> {
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: self.stream.stable_id(),
                stream_id: self.stream.stream_id(),
            })
            .await?;

        tracing::debug!("Terminated UniStreamReceiver");

        Ok(())
    }

    async fn receive_loop(&mut self) -> Result<(), TerminationError> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut header_read = false;
        let mut is_end = false;

        // If the received object is subgroup, store group id to judge the end of range.
        let mut subgroup_group_id: Option<u64> = None;

        loop {
            if is_end {
                break;
            }

            self.read_stream_and_add_to_buf().await?;

            is_end = self
                .read_buf_to_end_and_store_to_object_cache(
                    &mut header_read,
                    &mut object_cache_storage,
                    &mut subgroup_group_id,
                )
                .await?;
        }

        Ok(())
    }

    async fn read_stream_and_add_to_buf(&mut self) -> Result<(), TerminationError> {
        let mut buffer = vec![0; 65536].into_boxed_slice();

        let bytes_read: usize = match self.stream.read(&mut buffer).await {
            Ok(byte_read) => byte_read.unwrap(),
            Err(err) => {
                let msg = format!("Failed to read from stream: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        tracing::debug!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        self.buf.lock().await.extend_from_slice(&read_buf);

        Ok(())
    }

    async fn read_buf_to_end_and_store_to_object_cache(
        &mut self,
        header_read: &mut bool,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        subgroup_group_id: &mut Option<u64>,
    ) -> Result<bool, TerminationError> {
        loop {
            if !*header_read {
                match self
                    .read_header_from_buf_and_store_to_object_cache(object_cache_storage)
                    .await
                {
                    Ok((is_succeeded, received_header)) => {
                        if !is_succeeded {
                            break;
                        } else {
                            *header_read = true;

                            let received_header = received_header.unwrap();
                            self.set_rest_parameters_from_header(
                                received_header,
                                subgroup_group_id,
                            )
                            .await?;
                        }
                    }
                    Err(err) => {
                        let msg = format!("Fail to read header from buf: {:?}", err);
                        let code = TerminationErrorCode::InternalError;

                        return Err((code, msg));
                    }
                }
            }

            match self
                .read_object_stream_and_store_to_object_cache(object_cache_storage)
                .await
            {
                Ok(received_object) => {
                    if received_object.is_none() {
                        // return to read consequent data from stream
                        break;
                    }

                    let is_end = self
                        .judge_end_of_receiving(
                            received_object.as_ref().unwrap(),
                            subgroup_group_id,
                        )
                        .await?;

                    if is_end {
                        return Ok(true);
                    }
                }
                Err(err) => {
                    let msg = format!("Fail to read object stream from buf: {:?}", err);
                    let code = TerminationErrorCode::InternalError;

                    return Err((code, msg));
                }
            }
        }

        Ok(false)
    }

    async fn read_header_from_buf_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(bool, Option<CacheHeader>), TerminationError> {
        let result = self.try_to_read_buf_as_header(object_cache_storage).await;

        match result {
            StreamHeaderProcessResult::Success(received_header) => {
                let (subscribe_id, header_type) = match &received_header {
                    CacheHeader::Track(header) => {
                        let header_type = DataStreamType::StreamHeaderTrack;
                        let subscribe_id = header.subscribe_id();

                        (subscribe_id, header_type)
                    }
                    CacheHeader::Subgroup(header) => {
                        let header_type = DataStreamType::StreamHeaderSubgroup;
                        let subscribe_id = header.subscribe_id();

                        (subscribe_id, header_type)
                    }
                    _ => {
                        let msg = "received header not matched".to_string();
                        let code = TerminationErrorCode::InternalError;
                        return Err((code, msg));
                    }
                };

                match self
                    .open_downstream_uni_stream(subscribe_id, &header_type)
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        let msg = format!("Fail to open downstream uni stream: {:?}", err);
                        let code = TerminationErrorCode::InternalError;

                        return Err((code, msg));
                    }
                };
                Ok((true, Some(received_header)))
            }
            StreamHeaderProcessResult::IncompleteMessage => Ok((false, None)),
            StreamHeaderProcessResult::Failure(code, reason) => {
                let msg = std::format!("stream_header_read failure: {:?}", reason);
                Err((code, msg))
            }
        }
    }

    async fn try_to_read_buf_as_header(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> StreamHeaderProcessResult {
        let mut process_buf = self.buf.lock().await;
        let client = self.client.lock().await;

        let mut pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        stream_header_handler(
            &mut process_buf,
            &client,
            &mut pubsub_relation_manager,
            object_cache_storage,
        )
        .await
    }

    async fn open_downstream_uni_stream(
        &self,
        subscribe_id: u64,
        header_type: &DataStreamType,
    ) -> Result<()> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        let subscribers = pubsub_relation_manager
            .get_related_subscribers(self.stream.stable_id(), subscribe_id)
            .await?;

        for (downstream_session_id, downstream_subscribe_id) in subscribers {
            let open_downstream_stream_or_datagram_tx = self
                .senders
                .open_downstream_stream_or_datagram_txes()
                .lock()
                .await
                .get(&downstream_session_id)
                .unwrap()
                .clone();

            open_downstream_stream_or_datagram_tx
                .send((downstream_subscribe_id, *header_type))
                .await?;
        }

        Ok(())
    }

    async fn set_rest_parameters_from_header(
        &mut self,
        received_header: CacheHeader,
        subgroup_group_id: &mut Option<u64>,
    ) -> Result<(), TerminationError> {
        // Set subscribe_id and stream_header_type from received header
        let (subscribe_id, header_type) = match &received_header {
            CacheHeader::Track(header) => {
                let header_type = DataStreamType::StreamHeaderTrack;
                let subscribe_id = header.subscribe_id();

                (subscribe_id, header_type)
            }
            CacheHeader::Subgroup(header) => {
                let header_type = DataStreamType::StreamHeaderSubgroup;
                let subscribe_id = header.subscribe_id();

                (subscribe_id, header_type)
            }
            _ => {
                let msg = "received header not matched".to_string();
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };
        self.subscribe_id = Some(subscribe_id);
        self.stream_header_type = Some(header_type);

        // Set upstream_subscription from received header
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let upstream_subscription = match pubsub_relation_manager
            .get_upstream_subscription_by_ids(self.stream.stable_id(), subscribe_id)
            .await
        {
            Ok(upstream_subscription) => {
                if upstream_subscription.is_none() {
                    let msg = "Upstream subscription not found".to_string();
                    let code = TerminationErrorCode::InternalError;

                    return Err((code, msg));
                }

                upstream_subscription.unwrap()
            }

            Err(err) => {
                let msg = format!("Fail to get upstream subscription: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };
        self.upstream_subscription = Some(upstream_subscription);

        // Set group id if the received object is subgroup
        if let CacheHeader::Subgroup(header) = received_header {
            *subgroup_group_id = Some(header.group_id());
        }

        Ok(())
    }

    async fn read_object_stream_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<CacheObject>, TerminationError> {
        let result = self
            .try_to_read_buf_as_object_stream_and_store_to_object_cache(object_cache_storage)
            .await;

        match result {
            ObjectStreamProcessResult::Success(received_object) => Ok(Some(received_object)),
            ObjectStreamProcessResult::IncompleteMessage => Ok(None),
            ObjectStreamProcessResult::Failure(code, reason) => {
                let msg = std::format!("object_stream_read failure: {:?}", reason);
                Err((code, msg))
            }
        }
    }

    async fn try_to_read_buf_as_object_stream_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> ObjectStreamProcessResult {
        let mut process_buf = self.buf.lock().await;
        let client = self.client.lock().await;
        let upstream_subscribe_id = self.subscribe_id.unwrap();
        let stream_header_type = self.stream_header_type.unwrap();

        object_stream_handler(
            stream_header_type,
            upstream_subscribe_id,
            &mut process_buf,
            &client,
            object_cache_storage,
        )
        .await
    }

    async fn judge_end_of_receiving(
        &self,
        received_object: &CacheObject,
        subgroup_group_id: &Option<u64>,
    ) -> Result<bool, TerminationError> {
        let is_end_of_data_stream = self.judge_end_of_data_stream(received_object).await?;
        if is_end_of_data_stream {
            return Ok(true);
        }

        let upstream_subscription = self.upstream_subscription.as_ref().unwrap();
        let filter_type = upstream_subscription.get_filter_type();
        if filter_type == FilterType::AbsoluteRange {
            let is_end_of_absolute_range = self
                .judge_end_of_absolute_range(received_object, subgroup_group_id)
                .await?;
            if is_end_of_absolute_range {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn judge_end_of_data_stream(
        &self,
        received_object: &CacheObject,
    ) -> Result<bool, TerminationError> {
        let is_end = match received_object {
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
                let msg = "received object not matched".to_string();
                let code = TerminationErrorCode::InternalError;
                return Err((code, msg));
            }
        };

        Ok(is_end)
    }

    async fn judge_end_of_absolute_range(
        &self,
        received_object: &CacheObject,
        subgroup_group_id: &Option<u64>,
    ) -> Result<bool, TerminationError> {
        let upstream_subscription = self.upstream_subscription.as_ref().unwrap();
        let (end_group, end_object) = upstream_subscription.get_absolute_end();
        let end_group = end_group.unwrap();
        let end_object = end_object.unwrap();

        let is_end = match received_object {
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
                let msg = "received object not matched".to_string();
                let code = TerminationErrorCode::InternalError;
                return Err((code, msg));
            }
        };

        Ok(is_end)
    }
}

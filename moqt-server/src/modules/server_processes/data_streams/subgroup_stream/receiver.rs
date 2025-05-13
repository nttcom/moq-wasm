use super::uni_stream::UniRecvStream;
use crate::{
    modules::{
        buffer_manager::{request_buffer, BufferCommand},
        message_handlers::{
            subgroup_stream_header::{self, SubgroupStreamHeaderProcessResult},
            subgroup_stream_object::{self, SubgroupStreamObjectProcessResult},
        },
        moqt_client::MOQTClient,
        object_cache_storage::{
            cache::{CacheKey, SubgroupStreamId},
            wrapper::ObjectCacheStorageWrapper,
        },
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    signal_dispatcher::{DataStreamThreadSignal, SignalDispatcher, TerminateReason},
    TerminationError,
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{object_status::ObjectStatus, subgroup_stream},
    },
    models::{range::ObjectRange, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, Mutex},
    time::sleep,
};
use tracing::{self};

pub(crate) struct SubgroupStreamObjectReceiver {
    stream: UniRecvStream,
    buf: Arc<Mutex<BytesMut>>,
    senders: Arc<Senders>,
    client: Arc<Mutex<MOQTClient>>,
    duration: u64,
    signal_rx: Arc<Mutex<mpsc::Receiver<Box<DataStreamThreadSignal>>>>,
    subscribe_id: Option<u64>,
    subgroup_stream_id: Option<SubgroupStreamId>,
    filter_type: Option<FilterType>,
    requested_object_range: Option<ObjectRange>,
}

impl SubgroupStreamObjectReceiver {
    pub(crate) async fn init(
        stream: UniRecvStream,
        client: Arc<Mutex<MOQTClient>>,
        signal_rx: mpsc::Receiver<Box<DataStreamThreadSignal>>,
    ) -> Self {
        let senders = client.lock().await.senders();
        let stable_id = stream.stable_id();
        let stream_id = stream.stream_id();
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;
        // TODO: Set the accurate duration
        let duration = 100000;
        let signal_rx = Arc::new(Mutex::new(signal_rx));

        SubgroupStreamObjectReceiver {
            stream,
            buf,
            senders,
            client,
            duration,
            signal_rx,
            subscribe_id: None,
            subgroup_stream_id: None,
            filter_type: None,
            requested_object_range: None,
        }
    }

    pub(crate) async fn start(&mut self) -> Result<(), TerminationError> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut is_end = false;
        let session_id = self.client.lock().await.id();

        while !is_end {
            let signal_rx = self.signal_rx.clone();
            let mut signal_rx = signal_rx.lock().await;

            tokio::select! {
                read_bytes = self.read_stream() => {
                    self.add_to_buf(read_bytes?).await;

                    if !self.has_received_header() {
                        self.receive_header(session_id, &mut object_cache_storage)
                            .await?;

                        // If the header has not been received, continue to receive the header.
                        if !self.has_received_header() {
                            continue;
                        }
                    }

                    is_end = self.receive_objects(&mut object_cache_storage).await?;
                },
                Some(signal) = signal_rx.recv() => {
                    match *signal {
                        DataStreamThreadSignal::Terminate(reason) => {
                            tracing::debug!(
                                "Received Terminate signal stream_id: {:?}, group_id: {:?}, subgroup_id: {:?} (reason: {:?})",
                                self.stream.stream_id(),
                                self.subgroup_stream_id.unwrap().0,
                                self.subgroup_stream_id.unwrap().1,
                                reason
                            );
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn finish(self) -> Result<()> {
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: self.stream.stable_id(),
                stream_id: self.stream.stream_id(),
            })
            .await?;

        // Send STOP_SENDING frame to the publisher
        self.stream.stop();

        tracing::debug!("SubgroupStreamObjectReceiver finished");

        Ok(())
    }

    async fn read_stream(&mut self) -> Result<BytesMut, TerminationError> {
        // Align with the stream_receive_window configured on the MoQT Server
        let mut buffer = vec![0; 10 * 1024 * 1024].into_boxed_slice();

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

    fn has_received_header(&self) -> bool {
        self.subscribe_id.is_some()
    }

    async fn receive_header(
        &mut self,
        session_id: usize,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let header = match self.read_header_from_buf().await? {
            Some(header) => header,
            None => {
                return Ok(());
            }
        };

        let subscribe_id = self
            .get_subscribe_id(session_id, header.track_alias())
            .await?;
        self.subscribe_id = Some(subscribe_id);
        self.subgroup_stream_id = Some((header.group_id(), header.subgroup_id()));

        self.set_upstream_stream_id(session_id).await?;
        self.set_upstream_forwarding_preference(session_id).await?;

        let filter_type = self.get_upstream_filter_type(session_id).await?;
        self.filter_type = Some(filter_type);
        let requested_object_range = self.get_upstream_requested_object_range(session_id).await?;
        self.requested_object_range = Some(requested_object_range);

        self.create_cache_storage(session_id, header, object_cache_storage)
            .await?;

        self.create_forwarders(session_id).await?;

        Ok(())
    }

    async fn read_header_from_buf(
        &self,
    ) -> Result<Option<subgroup_stream::Header>, TerminationError> {
        let mut process_buf = self.buf.lock().await;
        let client = self.client.clone();

        let result = subgroup_stream_header::read_header(&mut process_buf, client).await;
        match result {
            SubgroupStreamHeaderProcessResult::Success(stream_header) => Ok(Some(stream_header)),
            SubgroupStreamHeaderProcessResult::Continue => Ok(None),
            SubgroupStreamHeaderProcessResult::Failure(code, reason) => {
                let msg = std::format!("stream_header_read failure: {:?}", reason);
                Err((code, msg))
            }
        }
    }

    async fn get_subscribe_id(
        &self,
        session_id: usize,
        track_alias: u64,
    ) -> Result<u64, TerminationError> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .get_upstream_subscribe_id_by_track_alias(session_id, track_alias)
            .await
        {
            Ok(Some(subscribe_id)) => Ok(subscribe_id),
            Ok(None) => {
                let msg = "Subscribe id is not found".to_string();
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
            Err(err) => {
                let msg = format!("Fail to get subscribe id: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn set_upstream_stream_id(
        &self,
        upstream_session_id: usize,
    ) -> Result<(), TerminationError> {
        // Register stream_id to send signal to other subgroup receiver threads in the same group
        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();
        let upstream_subscribe_id = self.subscribe_id.unwrap();
        let stream_id = self.stream.stream_id();

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .set_upstream_stream_id(
                upstream_session_id,
                upstream_subscribe_id,
                group_id,
                subgroup_id,
                stream_id,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to set upstream stream id: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn set_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
    ) -> Result<(), TerminationError> {
        let forwarding_preference = ForwardingPreference::Subgroup;
        let upstream_subscribe_id = self.subscribe_id.unwrap();

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

    async fn get_upstream_filter_type(
        &self,
        upstream_session_id: usize,
    ) -> Result<FilterType, TerminationError> {
        let upstream_subscribe_id = self.subscribe_id.unwrap();

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .get_upstream_filter_type(upstream_session_id, upstream_subscribe_id)
            .await
        {
            Ok(Some(filter_type)) => Ok(filter_type),
            Ok(None) => {
                let msg = "Filter type is not found".to_string();
                let code = TerminationErrorCode::InternalError;
                Err((code, msg))
            }
            Err(err) => {
                let msg = format!("Fail to get upstream filter type: {:?}", err);
                let code = TerminationErrorCode::InternalError;
                Err((code, msg))
            }
        }
    }

    async fn get_upstream_requested_object_range(
        &mut self,
        upstream_session_id: usize,
    ) -> Result<ObjectRange, TerminationError> {
        let upstream_subscribe_id = self.subscribe_id.unwrap();

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .get_upstream_requested_object_range(upstream_session_id, upstream_subscribe_id)
            .await
        {
            Ok(Some(range)) => Ok(range),
            Ok(None) => {
                let msg = "Requested range is not found".to_string();
                let code = TerminationErrorCode::InternalError;
                Err((code, msg))
            }
            Err(err) => {
                let msg = format!("Fail to get upstream requested range: {:?}", err);
                let code = TerminationErrorCode::InternalError;
                Err((code, msg))
            }
        }
    }

    async fn create_cache_storage(
        &self,
        upstream_session_id: usize,

        stream_header: subgroup_stream::Header,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let upstream_subscribe_id = self.subscribe_id.unwrap();
        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);

        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();
        let result = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, stream_header)
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to create cache storage: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn create_forwarders(&self, upstream_session_id: usize) -> Result<(), TerminationError> {
        let upstream_subscribe_id = self.subscribe_id.unwrap();

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
        let start_forwarder_txes = self.senders.start_forwarder_txes();
        let data_stream_type = DataStreamType::SubgroupHeader;

        let start_forwarder_tx = start_forwarder_txes
            .lock()
            .await
            .get(&downstream_session_id)
            .unwrap()
            .clone();

        start_forwarder_tx
            .send((
                downstream_subscribe_id,
                data_stream_type,
                self.subgroup_stream_id,
            ))
            .await?;

        Ok(())
    }

    async fn receive_objects(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<bool, TerminationError> {
        let session_id = self.client.lock().await.id();
        let subscribe_id = self.subscribe_id.unwrap();
        let mut is_end = false;

        while !is_end {
            is_end = match self
                .receive_object(session_id, subscribe_id, object_cache_storage)
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

        let is_data_stream_ended = self.is_data_stream_ended(&stream_object);

        if is_data_stream_ended {
            let stream_ids = self.get_stream_ids_for_same_group().await?;

            // Wait to forward rest of the objects on other receivers in the same group
            let send_delay_ms = Duration::from_millis(50); // FIXME: Temporary threshold
            sleep(send_delay_ms).await;

            for stream_id in stream_ids {
                // Skip the stream of this receiver
                if stream_id == self.stream.stream_id() {
                    continue;
                }
                self.send_termination_signal_to_receiver(&stream_object, stream_id)
                    .await?;
            }
        }

        let is_end = is_data_stream_ended;

        Ok(Some(is_end))
    }

    async fn read_object_from_buf(
        &self,
    ) -> Result<Option<subgroup_stream::Object>, TerminationError> {
        let mut buf = self.buf.lock().await;

        let result = subgroup_stream_object::read_object(&mut buf).await;

        match result {
            SubgroupStreamObjectProcessResult::Success(stream_object) => Ok(Some(stream_object)),
            SubgroupStreamObjectProcessResult::Continue => Ok(None),
        }
    }

    async fn store_object(
        &self,
        stream_object: &subgroup_stream::Object,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        let (group_id, subgroup_id) = self.subgroup_stream_id.unwrap();

        match object_cache_storage
            .set_subgroup_stream_object(
                &cache_key,
                group_id,
                subgroup_id,
                stream_object.clone(),
                self.duration,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!(
                    "Fail to store subgroup stream object to cache storage: {:?}",
                    err
                );
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfTrack, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    // TODO: Add handling for FIN message
    fn is_data_stream_ended(&self, stream_object: &subgroup_stream::Object) -> bool {
        matches!(
            stream_object.object_status(),
            Some(ObjectStatus::EndOfTrack)
                | Some(ObjectStatus::EndOfGroup)
                | Some(ObjectStatus::EndOfTrackAndGroup)
        )
    }

    async fn get_stream_ids_for_same_group(&self) -> Result<Vec<u64>, TerminationError> {
        let upstream_session_id = self.stream.stable_id();
        let upstream_subscribe_id = self.subscribe_id.unwrap();
        let (group_id, _) = self.subgroup_stream_id.unwrap();

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let subgroup_ids = match pubsub_relation_manager
            .get_upstream_subgroup_ids_for_group(
                upstream_session_id,
                upstream_subscribe_id,
                group_id,
            )
            .await
        {
            Ok(subgroup_ids) => subgroup_ids,
            Err(err) => {
                let msg = format!("Fail to get upstream subgroup ids by group: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                return Err((code, msg));
            }
        };

        let mut stream_ids: Vec<u64> = vec![];
        for subgroup_id in subgroup_ids {
            let stream_id = match pubsub_relation_manager
                .get_upstream_stream_id_for_subgroup(
                    upstream_session_id,
                    upstream_subscribe_id,
                    group_id,
                    subgroup_id,
                )
                .await
            {
                Ok(Some(stream_id)) => stream_id,
                Ok(None) => {
                    let msg = "Stream id is not found".to_string();
                    let code = TerminationErrorCode::InternalError;

                    return Err((code, msg));
                }
                Err(err) => {
                    let msg = format!("Fail to get upstream stream id by subgroup: {:?}", err);
                    let code = TerminationErrorCode::InternalError;

                    return Err((code, msg));
                }
            };

            stream_ids.push(stream_id);
        }

        Ok(stream_ids)
    }

    async fn send_termination_signal_to_receiver(
        &self,
        object: &subgroup_stream::Object,
        stream_id: u64,
    ) -> Result<(), TerminationError> {
        let upstream_session_id = self.stream.stable_id();
        let object_status = object.object_status().unwrap();

        let signal_dispatcher = SignalDispatcher::new(self.senders.signal_dispatch_tx().clone());

        tracing::debug!(
            "Send termination signal to upstream session: {}, stream: {}",
            upstream_session_id,
            stream_id
        );

        let terminate_reason = TerminateReason::ObjectStatus(object_status);
        let signal = Box::new(DataStreamThreadSignal::Terminate(terminate_reason));
        match signal_dispatcher
            .transfer_signal_to_data_stream_thread(upstream_session_id, stream_id, signal)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to send termination signal: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }
}

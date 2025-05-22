use super::uni_stream::UniSendStream;
use crate::{
    SubgroupStreamId,
    modules::{
        buffer_manager::BufferCommand,
        moqt_client::MOQTClient,
        object_cache_storage::{cache::CacheKey, wrapper::ObjectCacheStorageWrapper},
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    signal_dispatcher::{DataStreamThreadSignal, SignalDispatcher, TerminateReason},
};
use anyhow::{Ok, Result, bail};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{DataStreams, object_status::ObjectStatus, subgroup_stream},
    },
    models::{
        range::{ObjectRange, ObjectStart},
        tracks::ForwardingPreference,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{
    sync::{Arc, atomic::AtomicBool, atomic::Ordering},
    time::Duration,
};
use tokio::{
    sync::{Mutex, mpsc},
    task::{self, JoinHandle},
    time::sleep,
};
use tracing::{self};

pub(crate) struct SubgroupStreamObjectForwarder {
    stream: UniSendStream,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_track_alias: u64,
    cache_key: CacheKey,
    subgroup_stream_id: SubgroupStreamId,
    filter_type: FilterType,
    is_terminated: Arc<AtomicBool>,
    requested_object_range: ObjectRange,
    sleep_time: Duration,
    termination_task: Option<JoinHandle<()>>,
}

impl SubgroupStreamObjectForwarder {
    pub(crate) async fn init(
        stream: UniSendStream,
        downstream_subscribe_id: u64,
        client: Arc<Mutex<MOQTClient>>,
        subgroup_stream_id: SubgroupStreamId,
        mut signal_rx: mpsc::Receiver<Box<DataStreamThreadSignal>>,
    ) -> Result<Self> {
        let senders = client.lock().await.senders();
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

        let downstream_session_id = stream.stable_id();

        let downstream_track_alias = pubsub_relation_manager
            .get_downstream_track_alias(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        let filter_type = pubsub_relation_manager
            .get_downstream_filter_type(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        let requested_object_range = pubsub_relation_manager
            .get_downstream_requested_object_range(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        // Get the information of the original publisher who has the track being requested
        let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
            .get_related_publisher(downstream_session_id, downstream_subscribe_id)
            .await?;

        // Register stream_id to receive signal from other subgroup forwarder threads belong to the same group
        let (group_id, subgroup_id) = subgroup_stream_id;
        let stream_id = stream.stream_id();
        pubsub_relation_manager
            .set_downstream_stream_id(
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
                subgroup_id,
                stream_id,
            )
            .await?;

        // Task to receive termination signal
        let is_terminated = Arc::new(AtomicBool::new(false));
        let is_terminated_clone = is_terminated.clone();
        let client_id = client.lock().await.id();
        let termination_task = task::Builder::new()
            .name(&format!(
                "Object Stream Forwarder Terminator-{}-{}",
                client_id, stream_id
            ))
            .spawn(async move {
                if let Some(signal) = signal_rx.recv().await {
                    match *signal {
                        DataStreamThreadSignal::Terminate(reason) => {
                            tracing::debug!("Received Terminate signal (reason: {:?})", reason);
                            is_terminated_clone.store(true, Ordering::Relaxed);
                        }
                    }
                }
            })?;

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);

        let sleep_time = Duration::from_millis(5);
        let stream_object_forwarder = SubgroupStreamObjectForwarder {
            stream,
            senders,
            downstream_subscribe_id,
            downstream_track_alias,
            cache_key,
            subgroup_stream_id,
            filter_type,
            requested_object_range,
            is_terminated,
            sleep_time,
            termination_task: Some(termination_task),
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
        let upstream_header = self.get_upstream_header(object_cache_storage).await?;

        let downstream_header = self.generate_downstream_header(&upstream_header).await;

        let message_buf = self.packetize_header(&downstream_header).await?;
        self.send(message_buf).await?;

        Ok(())
    }

    async fn get_upstream_header(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<subgroup_stream::Header> {
        let (group_id, subgroup_id) = self.subgroup_stream_id;
        let subgroup_stream_header = object_cache_storage
            .get_subgroup_stream_header(&self.cache_key, group_id, subgroup_id)
            .await?;

        Ok(subgroup_stream_header)
    }

    async fn forward_objects(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<()> {
        let (is_end, first_object_cache_id) =
            self.forward_first_object(object_cache_storage).await?;
        tracing::debug!(
            "Forwarded first object: group_id: {:?}, subgroupid: {:?}",
            self.subgroup_stream_id.0,
            self.subgroup_stream_id.1
        );
        if is_end {
            // If the first object is EndOfTrack, EndOfGroup or EndOfTrackAndGroup,
            // we don't need to forward the rest of the objects.
            return Ok(());
        }

        self.forward_subsequent_objects(object_cache_storage, first_object_cache_id.unwrap())
            .await?;

        Ok(())
    }

    async fn forward_first_object(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(bool, Option<usize>)> {
        let object_cache_id = loop {
            if self.is_terminated.load(Ordering::Relaxed) {
                return Ok((true, None));
            }

            let first_stream_object = self.get_first_object(object_cache_storage).await?;
            let (cache_id, stream_object) = match first_stream_object {
                Some((id, object)) => (id, object),
                None => {
                    // If there is no object in the cache storage, sleep for a while and try again
                    sleep(self.sleep_time).await;
                    continue;
                }
            };

            // Packetize and send the object
            let message_buf = self.packetize_object(&stream_object).await?;
            self.send(message_buf).await?;

            // Check if the data stream has ended
            if self.is_data_stream_ended(&stream_object) {
                self.send_termination_signal_to_forwarders(stream_object.object_status().unwrap())
                    .await?;
                return Ok((true, None));
            }
            break cache_id;
        };
        Ok((false, Some(object_cache_id)))
    }

    async fn forward_subsequent_objects(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        mut object_cache_id: usize,
    ) -> Result<()> {
        loop {
            if self.is_terminated.load(Ordering::Relaxed) {
                break;
            }

            let stream_object = self
                .get_subsequent_object(object_cache_storage, object_cache_id)
                .await?;
            let (cache_id, stream_object) = match stream_object {
                Some((id, object)) => (id, object),
                None => {
                    // If there is no object in the cache storage, sleep for a while and try again
                    sleep(self.sleep_time).await;
                    continue;
                }
            };
            object_cache_id = cache_id;

            // Packetize and send the object
            let message_buf = self.packetize_object(&stream_object).await?;
            self.send(message_buf).await?;

            // Check if the data stream has ended
            if self.is_data_stream_ended(&stream_object) {
                self.send_termination_signal_to_forwarders(stream_object.object_status().unwrap())
                    .await?;
                return Ok(());
            }
        }

        Ok(())
    }

    async fn get_first_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let downstream_session_id = self.stream.stable_id();
        let downstream_subscribe_id = self.downstream_subscribe_id;

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let actual_object_start = pubsub_relation_manager
            .get_downstream_actual_object_start(downstream_session_id, downstream_subscribe_id)
            .await?;

        match actual_object_start {
            None => {
                // If there is no actual start, it means that this is the first forwarder on this subscription.
                let object_with_cache_id = self
                    .get_first_object_for_first_stream(object_cache_storage)
                    .await?;

                if object_with_cache_id.is_none() {
                    return Ok(None);
                }

                let (cache_id, stream_object) = object_with_cache_id.unwrap();
                let group_id = self.subgroup_stream_id.0;
                let object_id = stream_object.object_id();
                let actual_object_start = ObjectStart::new(group_id, object_id);

                pubsub_relation_manager
                    .set_downstream_actual_object_start(
                        downstream_session_id,
                        downstream_subscribe_id,
                        actual_object_start,
                    )
                    .await?;

                Ok(Some((cache_id, stream_object)))
            }
            Some(actual_object_start) => {
                // If there is an actual start, it means that this is the second or later forwarder on this subscription.
                self.get_first_object_for_subsequent_stream(
                    object_cache_storage,
                    actual_object_start,
                )
                .await
            }
        }
    }

    async fn get_first_object_for_first_stream(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let (group_id, subgroup_id) = self.subgroup_stream_id;

        match self.filter_type {
            FilterType::LatestGroup => {
                // TODO: Remove LatestGroup since it is not exist in the draft-10
                object_cache_storage
                    .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                    .await
            }
            FilterType::LatestObject => {
                // If the subscriber is the first subscriber for this track, the Relay needs to
                // start sending from first object for the subscriber to decode the contents.
                object_cache_storage
                    .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                let start_group_id = self.requested_object_range.start_group_id().unwrap();
                let start_object_id = self.requested_object_range.start_object_id().unwrap();

                if group_id == start_group_id {
                    object_cache_storage
                        .get_absolute_or_next_subgroup_stream_object(
                            &self.cache_key,
                            group_id,
                            subgroup_id,
                            start_object_id,
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

    async fn get_first_object_for_subsequent_stream(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        actual_object_start: ObjectStart,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let (group_id, subgroup_id) = self.subgroup_stream_id;

        if group_id == actual_object_start.group_id() {
            object_cache_storage
                .get_absolute_or_next_subgroup_stream_object(
                    &self.cache_key,
                    group_id,
                    subgroup_id,
                    actual_object_start.object_id(),
                )
                .await
        } else {
            object_cache_storage
                .get_first_subgroup_stream_object(&self.cache_key, group_id, subgroup_id)
                .await
        }
    }

    async fn get_subsequent_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        object_cache_id: usize,
    ) -> Result<Option<(usize, subgroup_stream::Object)>> {
        let (group_id, subgroup_id) = self.subgroup_stream_id;
        object_cache_storage
            .get_next_subgroup_stream_object(
                &self.cache_key,
                group_id,
                subgroup_id,
                object_cache_id,
            )
            .await
    }

    async fn generate_downstream_header(
        &self,
        upstream_header: &subgroup_stream::Header,
    ) -> subgroup_stream::Header {
        subgroup_stream::Header::new(
            self.downstream_track_alias, // Replace with downstream_track_alias
            upstream_header.group_id(),
            upstream_header.subgroup_id(),
            upstream_header.publisher_priority(),
        )
        .unwrap()
    }

    async fn packetize_header(&self, header: &subgroup_stream::Header) -> Result<BytesMut> {
        let downstream_session_id = self.stream.stable_id();
        let downstream_subscribe_id = self.downstream_subscribe_id;

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let downstream_track_alias = pubsub_relation_manager
            .get_downstream_track_alias(downstream_session_id, downstream_subscribe_id)
            .await?
            .unwrap();

        let header = subgroup_stream::Header::new(
            downstream_track_alias,
            header.group_id(),
            header.subgroup_id(),
            header.publisher_priority(),
        )
        .unwrap();

        let mut message_buf = BytesMut::new();
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::SubgroupHeader) as u64,
        ));
        header.packetize(&mut message_buf);

        Ok(message_buf)
    }

    async fn packetize_object(
        &mut self,
        stream_object: &subgroup_stream::Object,
    ) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        stream_object.packetize(&mut buf);

        Ok(buf)
    }

    async fn send(&mut self, message_buf: BytesMut) -> Result<()> {
        if let Err(e) = self.stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            bail!(e);
        }

        Ok(())
    }

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfTrack, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    fn is_data_stream_ended(&self, stream_object: &subgroup_stream::Object) -> bool {
        matches!(
            stream_object.object_status(),
            Some(ObjectStatus::EndOfTrack)
                | Some(ObjectStatus::EndOfGroup)
                | Some(ObjectStatus::EndOfTrackAndGroup)
        )
    }

    async fn get_stream_ids_for_same_group(&self) -> Result<Vec<u64>> {
        let downstream_session_id = self.stream.stable_id();
        let downstream_subscribe_id = self.downstream_subscribe_id;
        let (group_id, _) = self.subgroup_stream_id;

        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let subgroup_ids = pubsub_relation_manager
            .get_downstream_subgroup_ids_for_group(
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
            )
            .await?;

        let mut stream_ids: Vec<u64> = vec![];
        for subgroup_id in subgroup_ids {
            let stream_id = pubsub_relation_manager
                .get_downstream_stream_id_for_subgroup(
                    downstream_session_id,
                    downstream_subscribe_id,
                    group_id,
                    subgroup_id,
                )
                .await?
                .unwrap();

            stream_ids.push(stream_id);
        }

        Ok(stream_ids)
    }

    async fn send_termination_signal_to_forwarders(&self, reason: ObjectStatus) -> Result<()> {
        let signal_dispatcher = SignalDispatcher::new(self.senders.signal_dispatch_tx().clone());
        let terminate_reason = TerminateReason::ObjectStatus(reason);
        let signal = Box::new(DataStreamThreadSignal::Terminate(terminate_reason));

        let downstream_session_id = self.stream.stable_id();
        let stream_ids: Vec<u64> = {
            let same_group_stream_ids = self.get_stream_ids_for_same_group().await?;
            same_group_stream_ids
                .into_iter()
                .filter(|&stream_id| stream_id != self.stream.stream_id())
                .collect()
        };

        // Wait to forward rest of the objects on other forwarders in the same group
        let send_delay_ms = Duration::from_millis(50); // FIXME: Temporary threshold
        sleep(send_delay_ms).await;

        for stream_id in stream_ids {
            signal_dispatcher
                .transfer_signal_to_data_stream_thread(
                    downstream_session_id,
                    stream_id,
                    signal.clone(),
                )
                .await?;
        }

        Ok(())
    }

    pub(crate) async fn finish(&mut self) -> Result<()> {
        if let Some(termination_task) = self.termination_task.take() {
            termination_task.abort();
            if let Err(e) = termination_task.await {
                if e.is_cancelled() {
                    tracing::debug!("Termination task was aborted.");
                } else {
                    tracing::error!("Unexpected error while awaiting termination task: {:?}", e);
                }
            }
        }

        let downstream_session_id = self.stream.stable_id();
        let downstream_stream_id = self.stream.stream_id();
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: downstream_session_id,
                stream_id: downstream_stream_id,
            })
            .await?;

        // Send RESET_STREAM frame to the subscriber
        self.stream.finish().await?;

        tracing::info!("SubgroupStreamObjectForwarder finished");

        Ok(())
    }
}

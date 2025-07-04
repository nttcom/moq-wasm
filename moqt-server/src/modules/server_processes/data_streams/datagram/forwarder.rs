use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::{cache::CacheKey, wrapper::ObjectCacheStorageWrapper},
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{Ok, Result, bail};
use bytes::BytesMut;
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::{
        control_messages::subscribe::FilterType,
        data_streams::{
            DataStreams, DatagramObject, datagram, datagram_status, object_status::ObjectStatus,
        },
    },
    models::{range::ObjectRange, tracks::ForwardingPreference},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::sleep};
use tracing::{self};
use wtransport::Connection;

pub(crate) struct DatagramObjectForwarder {
    session: Arc<Connection>,
    senders: Arc<Senders>,
    downstream_subscribe_id: u64,
    downstream_track_alias: u64,
    cache_key: CacheKey,
    filter_type: FilterType,
    requested_object_range: ObjectRange,
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

        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);

        let datagram_object_forwarder = DatagramObjectForwarder {
            session,
            senders,
            downstream_subscribe_id,
            downstream_track_alias,
            cache_key,
            filter_type,
            requested_object_range,
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
            let (cache_id, upstream_object) = match self
                .get_upstream_object(object_cache_storage, cache_id)
                .await?
            {
                Some((id, object)) => (id, object),
                None => {
                    // If there is no object in the cache storage, sleep for a while and try again
                    sleep(self.sleep_time).await;
                    continue;
                }
            };

            let downstream_object = self.generate_downstream_object(&upstream_object);

            let message_buf = self.packetize(&downstream_object).await?;
            self.send(message_buf).await?;

            let mut is_end = false;
            if let DatagramObject::ObjectDatagramStatus(object) = &downstream_object {
                is_end = self.is_data_stream_ended(object);
            }

            return Ok((Some(cache_id), is_end));
        }
    }

    async fn get_upstream_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        cache_id: Option<usize>,
    ) -> Result<Option<(usize, DatagramObject)>> {
        let cache = match cache_id {
            // Try to get the first object according to Filter Type
            None => self.get_first_object(object_cache_storage).await?,
            Some(cache_id) => {
                // Try to get the subsequent object with cache_id
                self.get_subsequent_object(object_cache_storage, cache_id)
                    .await?
            }
        };

        match cache {
            None => Ok(None),
            Some(object_with_cache_id) => Ok(Some(object_with_cache_id)),
        }
    }

    async fn get_first_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<Option<(usize, DatagramObject)>> {
        match self.filter_type {
            // TODO: Remove LatestGroup since it is not exist in the draft-10
            FilterType::LatestGroup => {
                object_cache_storage
                    .get_latest_datagram_group(&self.cache_key)
                    .await
            }
            FilterType::LatestObject => {
                object_cache_storage
                    .get_latest_datagram_object(&self.cache_key)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                let start_group = self.requested_object_range.start_group_id().unwrap();
                let start_object = self.requested_object_range.start_object_id().unwrap();

                object_cache_storage
                    .get_absolute_datagram_object(&self.cache_key, start_group, start_object)
                    .await
            }
        }
    }

    async fn get_subsequent_object(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
        object_cache_id: usize,
    ) -> Result<Option<(usize, DatagramObject)>> {
        object_cache_storage
            .get_next_datagram_object(&self.cache_key, object_cache_id)
            .await
    }

    fn generate_downstream_object(&self, upstream_object: &DatagramObject) -> DatagramObject {
        match upstream_object {
            DatagramObject::ObjectDatagram(object) => {
                let object_datagram = self.generate_downstream_object_datagram(object);
                DatagramObject::ObjectDatagram(object_datagram)
            }
            DatagramObject::ObjectDatagramStatus(object) => {
                let object_datagram_status =
                    self.generate_downstream_object_datagram_status(object);
                DatagramObject::ObjectDatagramStatus(object_datagram_status)
            }
        }
    }

    fn generate_downstream_object_datagram(
        &self,
        upstream_object: &datagram::Object,
    ) -> datagram::Object {
        let extension_headers = upstream_object.extension_headers().clone();
        datagram::Object::new(
            self.downstream_track_alias, // Replace with downstream_track_alias
            upstream_object.group_id(),
            upstream_object.object_id(),
            upstream_object.publisher_priority(),
            extension_headers,
            upstream_object.object_payload(),
        )
        .unwrap()
    }

    fn generate_downstream_object_datagram_status(
        &self,
        upstream_object: &datagram_status::Object,
    ) -> datagram_status::Object {
        let extension_headers = upstream_object.extension_headers().clone();
        datagram_status::Object::new(
            self.downstream_track_alias, // Replace with downstream_track_alias
            upstream_object.group_id(),
            upstream_object.object_id(),
            upstream_object.publisher_priority(),
            extension_headers,
            upstream_object.object_status(),
        )
        .unwrap()
    }

    async fn packetize(&mut self, downstream_object: &DatagramObject) -> Result<BytesMut> {
        match downstream_object {
            DatagramObject::ObjectDatagram(object) => self.packetize_object_datagram(object).await,
            DatagramObject::ObjectDatagramStatus(object) => {
                self.packetize_object_datagram_status(object).await
            }
        }
    }

    async fn packetize_object_datagram(
        &mut self,
        datagram_object: &datagram::Object,
    ) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        datagram_object.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::ObjectDatagram) as u64,
        ));
        message_buf.extend(buf);

        Ok(message_buf)
    }

    async fn packetize_object_datagram_status(
        &mut self,
        datagram_object: &datagram_status::Object,
    ) -> Result<BytesMut> {
        let mut buf = BytesMut::new();
        datagram_object.packetize(&mut buf);

        let mut message_buf = BytesMut::with_capacity(buf.len());
        message_buf.extend(write_variable_integer(
            u8::from(DataStreamType::ObjectDatagramStatus) as u64,
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

    // This function is implemented according to the following sentence in draft.
    //   A relay MAY treat receipt of EndOfGroup, EndOfTrack, GroupDoesNotExist, or
    //   EndOfTrack objects as a signal to close corresponding streams even if the FIN
    //   has not arrived, as further objects on the stream would be a protocol violation.
    fn is_data_stream_ended(&self, object: &datagram_status::Object) -> bool {
        matches!(
            object.object_status(),
            ObjectStatus::EndOfTrack | ObjectStatus::EndOfTrackAndGroup
        )
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
}

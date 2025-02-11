use crate::{
    modules::{
        buffer_manager::request_buffer,
        message_handlers::datagram_object::{self, DatagramObjectProcessResult},
        moqt_client::MOQTClient,
        object_cache_storage::{cache::CacheKey, wrapper::ObjectCacheStorageWrapper},
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    TerminationError,
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode, data_stream_type::DataStreamType,
    messages::data_streams::datagram, models::tracks::ForwardingPreference,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self};
use wtransport::datagram::Datagram;

pub(crate) struct DatagramObjectReceiver {
    buf: Arc<Mutex<BytesMut>>,
    senders: Arc<Senders>,
    client: Arc<Mutex<MOQTClient>>,
    duration: u64,
}

impl DatagramObjectReceiver {
    pub(crate) async fn init(client: Arc<Mutex<MOQTClient>>) -> Self {
        let senders = client.lock().await.senders();
        let stable_id = client.lock().await.id();
        let stream_id = 0; // stream_id of datagram does not exist (TODO: delete buffer manager)
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;
        // TODO: Set the accurate duration
        let duration = 100000;

        DatagramObjectReceiver {
            buf,
            senders,
            client,
            duration,
        }
    }

    pub(crate) async fn receive_object(
        &mut self,
        datagram: Datagram,
    ) -> Result<(), TerminationError> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let payload = datagram.payload();
        let read_bytes = BytesMut::from(&payload[..]);
        self.add_to_buf(read_bytes).await;

        let object = match self.read_object_from_buf().await? {
            Some(object) => object,
            None => {
                return Ok(());
            }
        };

        let session_id = self.client.lock().await.id();
        let subscribe_id = object.subscribe_id();

        if self
            .is_first_object(session_id, subscribe_id, &mut object_cache_storage)
            .await?
        {
            self.set_upstream_forwarding_preference(session_id, subscribe_id)
                .await?;

            self.create_cache_storage(session_id, subscribe_id, &mut object_cache_storage)
                .await?;

            self.store_object(object, session_id, subscribe_id, &mut object_cache_storage)
                .await?;

            self.create_forwarders(session_id, subscribe_id).await?;
        } else {
            self.store_object(object, session_id, subscribe_id, &mut object_cache_storage)
                .await?;
        }

        Ok(())
    }

    async fn add_to_buf(&mut self, read_bytes: BytesMut) {
        let mut buf = self.buf.lock().await;
        buf.extend_from_slice(&read_bytes);
    }

    async fn read_object_from_buf(&self) -> Result<Option<datagram::Object>, TerminationError> {
        let result = self.try_read_object_from_buf().await;

        match result {
            DatagramObjectProcessResult::Success(datagram_object) => Ok(Some(datagram_object)),
            DatagramObjectProcessResult::Continue => Ok(None),
            DatagramObjectProcessResult::Failure(code, reason) => {
                let msg = std::format!("stream_object_read failure: {:?}", reason);
                tracing::error!(msg);
                Err((code, reason))
            }
        }
    }

    async fn try_read_object_from_buf(&self) -> DatagramObjectProcessResult {
        let mut buf = self.buf.lock().await;
        let client = self.client.clone();

        datagram_object::try_read_object(&mut buf, client).await
    }

    async fn is_first_object(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<bool, TerminationError> {
        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        match object_cache_storage.exist_datagram_cache(&cache_key).await {
            Ok(exist) => {
                let is_first_object = !exist;
                Ok(is_first_object)
            }
            _ => {
                let msg = "Unexpected header cache is already set".to_string();
                let code = TerminationErrorCode::InternalError;
                Err((code, msg))
            }
        }
    }

    async fn set_upstream_forwarding_preference(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
    ) -> Result<(), TerminationError> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        match pubsub_relation_manager
            .set_upstream_forwarding_preference(
                upstream_session_id,
                upstream_subscribe_id,
                ForwardingPreference::Datagram,
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

    async fn create_cache_storage(
        &self,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        match object_cache_storage.create_datagram_cache(&cache_key).await {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = format!("Fail to create cache storage: {:?}", err);
                let code = TerminationErrorCode::InternalError;

                Err((code, msg))
            }
        }
    }

    async fn store_object(
        &self,
        datagram_object: datagram::Object,
        upstream_session_id: usize,
        upstream_subscribe_id: u64,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        let cache_key = CacheKey::new(upstream_session_id, upstream_subscribe_id);
        match object_cache_storage
            .set_datagram_object(&cache_key, datagram_object, self.duration)
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
        let start_forwarder_txes = self.senders.start_forwarder_txes();
        let data_stream_type = DataStreamType::ObjectDatagram;

        let start_forwarder_tx = start_forwarder_txes
            .lock()
            .await
            .get(&downstream_session_id)
            .unwrap()
            .clone();

        start_forwarder_tx
            .send((downstream_subscribe_id, data_stream_type, None))
            .await?;

        Ok(())
    }
}

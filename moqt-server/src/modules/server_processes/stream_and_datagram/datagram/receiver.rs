use crate::{
    modules::{
        buffer_manager::request_buffer,
        message_handlers::object_datagram::{object_datagram_handler, ObjectDatagramProcessResult},
        moqt_client::MOQTClient,
        object_cache_storage::{CacheObject, ObjectCacheStorageWrapper},
        pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
        server_processes::senders::Senders,
    },
    TerminationError,
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode, data_stream_type::DataStreamType,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self};
use wtransport::datagram::Datagram;

pub(crate) struct DatagramReceiver {
    buf: Arc<Mutex<BytesMut>>,
    senders: Arc<Senders>,
    client: Arc<Mutex<MOQTClient>>,
}

impl DatagramReceiver {
    pub(crate) async fn init(client: Arc<Mutex<MOQTClient>>) -> Self {
        let senders = client.lock().await.senders();
        let stable_id = client.lock().await.id();
        let stream_id = 0; // stream_id of datagram does not exist (TODO: delete buffer manager)
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;

        DatagramReceiver {
            buf,
            senders,
            client,
        }
    }

    pub(crate) async fn receive(&mut self, datagram: Datagram) -> Result<(), TerminationError> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        self.add_datagram_to_buf(datagram).await;

        self.read_buf_and_store_to_object_cache(&mut object_cache_storage)
            .await?;

        Ok(())
    }

    async fn add_datagram_to_buf(&mut self, datagram: Datagram) {
        let datagram_payload = datagram.payload();

        let read_buf = BytesMut::from(&datagram_payload[..]);
        self.buf.lock().await.extend_from_slice(&read_buf);
    }

    async fn read_buf_and_store_to_object_cache(
        &mut self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(), TerminationError> {
        loop {
            let is_succeeded = self
                .read_object_datagram_and_store_to_object_cache(object_cache_storage)
                .await?;

            if !is_succeeded {
                break;
            }
        }

        Ok(())
    }

    async fn read_object_datagram_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<bool, TerminationError> {
        let result = self
            .try_to_read_buf_and_store_to_object_cache(object_cache_storage)
            .await;

        match result {
            ObjectDatagramProcessResult::Success((received_object, is_first_time)) => {
                if is_first_time {
                    if let CacheObject::Datagram(received_object) = received_object {
                        match self
                            .open_downstream_datagram(received_object.subscribe_id())
                            .await
                        {
                            Ok(_) => {}
                            Err(err) => {
                                let msg = format!("Fail to open downstream datagram: {:?}", err);
                                let code = TerminationErrorCode::InternalError;

                                return Err((code, msg));
                            }
                        };
                    }
                }
                Ok(true)
            }

            ObjectDatagramProcessResult::IncompleteMessage => Ok(false),
            ObjectDatagramProcessResult::Failure(code, reason) => {
                let msg = std::format!("object_stream_read failure: {:?}", reason);
                tracing::error!(msg);
                Err((code, reason))
            }
        }
    }

    async fn open_downstream_datagram(&self, upstream_subscribe_id: u64) -> Result<()> {
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());
        let upstream_session_id = self.client.lock().await.id();
        let open_subscription_txes = self.senders.open_downstream_stream_or_datagram_txes();
        let header_type = DataStreamType::ObjectDatagram;

        let subscribers = pubsub_relation_manager
            .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
            .await
            .unwrap();

        for (downstream_session_id, downstream_subscribe_id) in subscribers {
            let open_subscription_tx = open_subscription_txes
                .lock()
                .await
                .get(&downstream_session_id)
                .unwrap()
                .clone();

            let _ = open_subscription_tx
                .send((downstream_subscribe_id, header_type))
                .await;
        }
        Ok(())
    }

    async fn try_to_read_buf_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> ObjectDatagramProcessResult {
        let mut process_buf = self.buf.lock().await;
        let client = self.client.clone();
        let mut pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(self.senders.pubsub_relation_tx().clone());

        object_datagram_handler(
            &mut process_buf,
            client,
            &mut pubsub_relation_manager,
            object_cache_storage,
        )
        .await
    }
}

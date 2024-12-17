use super::streams::UniRecvStream;
use crate::modules::{
    buffer_manager::{request_buffer, BufferCommand},
    message_handlers::{
        object_stream::{object_stream_handler, ObjectStreamProcessResult},
        stream_header::{stream_header_handler, StreamHeaderProcessResult},
    },
    moqt_client::MOQTClient,
    object_cache_storage::ObjectCacheStorageWrapper,
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    server_processes::senders::Senders,
};
use anyhow::{bail, Result};
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode, data_stream_type::DataStreamType,
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
}

impl UniStreamReceiver {
    pub(crate) async fn start(stream: UniRecvStream, client: Arc<Mutex<MOQTClient>>) -> Result<()> {
        let mut uni_stream_receiver = Self::init(stream, client).await;

        uni_stream_receiver.receive_loop().await?;

        uni_stream_receiver.terminate().await?;

        Ok(())
    }

    async fn init(stream: UniRecvStream, client: Arc<Mutex<MOQTClient>>) -> Self {
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
        }
    }

    async fn receive_loop(&mut self) -> Result<()> {
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(self.senders.object_cache_tx().clone());

        let mut header_read = false;

        loop {
            self.read_stream_and_add_to_buf().await?;

            self.read_buf_to_end_and_store_to_object_cache(
                &mut header_read,
                &mut object_cache_storage,
            )
            .await?;
        }
    }

    async fn read_stream_and_add_to_buf(&mut self) -> Result<()> {
        let mut buffer = vec![0; 65536].into_boxed_slice();

        let bytes_read: usize = match self.stream.read(&mut buffer).await {
            Ok(byte_read) => byte_read.unwrap(),
            Err(err) => {
                let msg = format!("Failed to read from stream: {:?}", err);
                tracing::error!(msg);
                let _ = self
                    .senders
                    .close_session_tx()
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.clone(),
                    ))
                    .await;
                bail!(msg);
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
    ) -> Result<()> {
        loop {
            if !*header_read {
                match self
                    .read_header_from_buf_and_store_to_object_cache(object_cache_storage)
                    .await
                {
                    Ok((is_succeeded, header_params)) => {
                        if !is_succeeded {
                            break;
                        } else {
                            *header_read = true;

                            let (subscribe_id, header_type) = header_params.unwrap();

                            self.subscribe_id = Some(subscribe_id);
                            self.stream_header_type = Some(header_type);
                        }
                    }
                    Err(err) => {
                        let msg = format!("Fail to read header from buf: {:?}", err);
                        tracing::error!(msg);
                        bail!(msg);
                    }
                }
            }

            match self
                .read_object_stream_and_store_to_object_cache(object_cache_storage)
                .await
            {
                Ok(is_succeeded) => {
                    if !is_succeeded {
                        break;
                    }
                }
                Err(err) => {
                    let msg = format!("Fail to read object stream from buf: {:?}", err);
                    tracing::error!(msg);
                    bail!(msg);
                }
            }
        }

        Ok(())
    }

    async fn read_header_from_buf_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<(bool, Option<(u64, DataStreamType)>)> {
        let result = self.try_to_read_buf_as_header(object_cache_storage).await;

        match result {
            StreamHeaderProcessResult::Success((subscribe_id, header_type)) => {
                self.open_downstream_uni_stream(subscribe_id, &header_type)
                    .await?;
                Ok((true, Some((subscribe_id, header_type))))
            }
            StreamHeaderProcessResult::IncompleteMessage => Ok((false, None)),
            StreamHeaderProcessResult::Failure(code, reason) => {
                let msg = std::format!("stream_header_read failure: {:?}", reason);
                tracing::error!(msg);
                self.senders
                    .close_session_tx()
                    .send((u8::from(code) as u64, reason))
                    .await?;
                bail!(msg);
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
            .await
            .unwrap();

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

    async fn read_object_stream_and_store_to_object_cache(
        &self,
        object_cache_storage: &mut ObjectCacheStorageWrapper,
    ) -> Result<bool> {
        let result = self
            .try_to_read_buf_as_object_stream_and_store_to_object_cache(object_cache_storage)
            .await;

        match result {
            ObjectStreamProcessResult::Success => Ok(true),
            ObjectStreamProcessResult::IncompleteMessage => Ok(false),
            ObjectStreamProcessResult::Failure(code, reason) => {
                let msg = std::format!("object_stream_read failure: {:?}", reason);
                tracing::error!(msg);
                self.senders
                    .close_session_tx()
                    .send((u8::from(code) as u64, reason))
                    .await?;
                bail!(msg)
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

    async fn terminate(&self) -> Result<()> {
        self.senders
            .buffer_tx()
            .send(BufferCommand::ReleaseStream {
                session_id: self.stream.stable_id(),
                stream_id: self.stream.stream_id(),
            })
            .await?;

        self.senders
            .open_downstream_stream_or_datagram_txes()
            .lock()
            .await
            .remove(&self.stream.stable_id());

        Ok(())
    }
}

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
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::{
    constants::TerminationErrorCode, data_stream_type::DataStreamType,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self, Instrument};

pub(crate) async fn handle_uni_recv_stream(
    stream: &mut UniRecvStream,
    client: Arc<Mutex<MOQTClient>>,
) -> Result<()> {
    let mut header_read = false;

    let senders = client.lock().await.senders();
    let stable_id = stream.stable_id();
    let mut upstream_subscribe_id: u64 = 0;
    let mut stream_header_type: DataStreamType = DataStreamType::ObjectDatagram;
    let stream_id = stream.stream_id();
    let shared_recv_stream = &mut stream.shared_recv_stream();
    let shared_recv_stream = Arc::clone(shared_recv_stream);

    let mut pubsub_relation_manager =
        PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());

    let mut object_cache_storage =
        ObjectCacheStorageWrapper::new(senders.object_cache_tx().clone());

    let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;
    let buf_clone = Arc::clone(&buf);
    let senders_clone = Arc::clone(&senders);

    // Loop for reading the stream
    tokio::spawn(
        async move {
            loop {
                let mut buffer = vec![0; 65536].into_boxed_slice();
                let mut recv_stream = shared_recv_stream.lock().await;

                let bytes_read: usize = match recv_stream.read(&mut buffer).await {
                    Ok(byte_read) => byte_read.unwrap(),
                    Err(err) => {
                        tracing::error!("Failed to read from stream");
                        let _ = senders_clone
                            .close_session_tx()
                            .send((
                                u8::from(TerminationErrorCode::InternalError) as u64,
                                err.to_string(),
                            ))
                            .await;
                        break;
                    }
                };

                tracing::debug!("bytes_read: {}", bytes_read);

                let read_buf = BytesMut::from(&buffer[..bytes_read]);

                {
                    buf_clone.lock().await.extend_from_slice(&read_buf);
                }

                tracing::debug!("buf size: {}", buf_clone.lock().await.len());
            }
        }
        .in_current_span(),
    );

    loop {
        if !header_read {
            // Read header

            let result: StreamHeaderProcessResult;
            {
                let mut process_buf = buf.lock().await;
                let client = client.lock().await;

                result = stream_header_handler(
                    &mut process_buf,
                    &client,
                    &mut pubsub_relation_manager,
                    &mut object_cache_storage,
                )
                .await;
            }

            match result {
                StreamHeaderProcessResult::Success((subscribe_id, header_type)) => {
                    tracing::trace!("stream_header_read success");
                    upstream_subscribe_id = subscribe_id;
                    stream_header_type = header_type.clone();

                    // Open send uni-directional stream for subscribers
                    let subscribers = pubsub_relation_manager
                        .get_related_subscribers(stable_id, upstream_subscribe_id)
                        .await
                        .unwrap();

                    for (downstream_session_id, downstream_subscribe_id) in subscribers {
                        let open_downstream_stream_or_datagram_tx = senders
                            .open_downstream_stream_or_datagram_txes()
                            .lock()
                            .await
                            .get(&downstream_session_id)
                            .unwrap()
                            .clone();

                        open_downstream_stream_or_datagram_tx
                            .send((downstream_subscribe_id, stream_header_type.clone()))
                            .await?;
                    }

                    header_read = true;
                }
                StreamHeaderProcessResult::Continue => {
                    tracing::trace!("retry stream_header_read");
                    continue;
                }
                StreamHeaderProcessResult::Failure(code, message) => {
                    tracing::error!("stream_header_read failure: {:?}", message);
                    senders
                        .close_session_tx()
                        .send((u8::from(code) as u64, message))
                        .await?;
                    break;
                }
            }
        }

        let result: ObjectStreamProcessResult;

        // Read Object Stream
        {
            let mut process_buf = buf.lock().await;
            let client = client.lock().await;

            result = object_stream_handler(
                stream_header_type.clone(),
                upstream_subscribe_id,
                &mut process_buf,
                &client,
                &mut object_cache_storage,
            )
            .await;
        }

        match result {
            ObjectStreamProcessResult::Success => {
                tracing::trace!("object_stream_read success");
            }
            ObjectStreamProcessResult::Continue => {
                tracing::trace!("retry object_stream_read");
                continue;
            }
            ObjectStreamProcessResult::Failure(code, message) => {
                tracing::error!("object_stream_read failure: {:?}", message);
                senders
                    .close_session_tx()
                    .send((u8::from(code) as u64, message))
                    .await?;
                break;
            }
        }
    }

    senders
        .buffer_tx()
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    senders
        .open_downstream_stream_or_datagram_txes()
        .lock()
        .await
        .remove(&stable_id);

    Ok(())
}

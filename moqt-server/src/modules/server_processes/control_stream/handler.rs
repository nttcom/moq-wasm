use super::bi_stream::BiStream;
use crate::modules::{
    buffer_manager::{BufferCommand, request_buffer},
    control_message_dispatcher::ControlMessageDispatcher,
    message_handlers::control_message::{MessageProcessResult, control_message_handler},
    moqt_client::MOQTClient,
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
};
use anyhow::Result;
use bytes::BytesMut;
use moqt_core::constants::UnderlayType;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{self};

pub(crate) async fn handle_control_stream(
    stream: &mut BiStream,
    client: Arc<Mutex<MOQTClient>>,
) -> Result<()> {
    let senders = client.lock().await.senders();
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id();
    let stream_id = stream.stream_id();
    let recv_stream = &mut stream.recv_stream;
    let shared_send_stream = &mut stream.shared_send_stream;

    let mut pubsub_relation_manager =
        PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());
    let mut control_message_dispatcher =
        ControlMessageDispatcher::new(senders.control_message_dispatch_tx().clone());

    let mut object_cache_storage =
        crate::modules::object_cache_storage::wrapper::ObjectCacheStorageWrapper::new(
            senders.object_cache_tx().clone(),
        );

    loop {
        let bytes_read = match recv_stream.read(&mut buffer).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::debug!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = request_buffer(senders.buffer_tx().clone(), stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf);

        let message_result: MessageProcessResult;
        {
            let mut client = client.lock().await;
            // TODO: Move the implementation of control_message_handler to the server side since it is only used by the server
            message_result = control_message_handler(
                &mut buf,
                UnderlayType::WebTransport,
                &mut client,
                senders.start_forwarder_txes().clone(),
                &mut pubsub_relation_manager,
                &mut control_message_dispatcher,
                &mut object_cache_storage,
            )
            .await;
        }

        tracing::debug!("message_result: {:?}", message_result);

        match message_result {
            MessageProcessResult::Success(buf) => {
                let mut shared_send_stream = shared_send_stream.lock().await;
                shared_send_stream.write_all(&buf).await?;

                tracing::info!("Message is sent.");
                tracing::debug!("sent message: {:x?}", buf.to_vec());
            }
            MessageProcessResult::SuccessWithoutResponse => {}
            MessageProcessResult::Failure(code, message) => {
                senders
                    .close_session_tx()
                    .send((u8::from(code) as u64, message))
                    .await?;
                break;
            }
            MessageProcessResult::Fragment => (),
        };
    }

    senders
        .buffer_tx()
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    Ok(())
}

use super::stream_and_datagram::{
    bi_directional_stream::{
        forwarder::forward_control_message, receiver::handle_bi_recv_stream, stream::BiStream,
    },
    datagram::{forwarder::DatagramForwarder, receiver::DatagramReceiver},
    uni_directional_stream::{
        forwarder::ObjectStreamForwarder,
        receiver::UniStreamReceiver,
        streams::{UniRecvStream, UniSendStream},
    },
};
use crate::modules::{moqt_client::MOQTClient, send_stream_dispatcher::SendStreamDispatchCommand};
use anyhow::{bail, Result};
use moqt_core::{
    constants::{StreamDirection, TerminationErrorCode},
    data_stream_type::DataStreamType,
    messages::moqt_payload::MOQTPayload,
};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{self, Instrument};
use wtransport::{datagram::Datagram, Connection, RecvStream, SendStream};

async fn spawn_bi_stream_threads(
    client: Arc<Mutex<MOQTClient>>,
    send_stream: SendStream,
    recv_stream: RecvStream,
    is_control_stream_opened: &mut bool,
) -> Result<()> {
    let senders = client.lock().await.senders();

    if *is_control_stream_opened {
        tracing::error!("Control stream already opened");
        senders
            .close_session_tx()
            .send((
                u8::from(TerminationErrorCode::ProtocolViolation) as u64,
                "Control stream already opened".to_string(),
            ))
            .await?;
        bail!("Control stream already opened");
    }
    *is_control_stream_opened = true;

    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Accepted BI stream");
    });

    let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
    senders
        .send_stream_tx()
        .send(SendStreamDispatchCommand::Set {
            session_id: stable_id,
            stream_direction: StreamDirection::Bi,
            sender: message_tx,
        })
        .await?;

    // The send_stream is wrapped with a Mutex to make it thread-safe since it can be called from multiple threads for returning and forwarding messages.
    let shared_send_stream = Arc::new(Mutex::new(send_stream));

    // Spawn thread listenning for WebTransport messages
    let send_stream = Arc::clone(&shared_send_stream);
    let session_span = session_span.clone();
    let stream_id = recv_stream.id().into_u64();
    tokio::spawn(
        async move {
            let mut stream = BiStream::new(stable_id, stream_id, recv_stream, send_stream);
            handle_bi_recv_stream(&mut stream, client)
                .instrument(session_span)
                .await
        }
        .in_current_span(),
    );

    // Thread to forward messages (ANNOUNCE SUBSCRIBE) from the server
    let send_stream = Arc::clone(&shared_send_stream);
    tokio::spawn(
        async move {
            let session_span = tracing::info_span!("Session", stable_id);
            forward_control_message(send_stream, message_rx)
                .instrument(session_span)
                .await;
        }
        .in_current_span(),
    );

    Ok(())
}

async fn spawn_uni_recv_stream_thread(
    client: Arc<Mutex<MOQTClient>>,
    recv_stream: RecvStream,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Accepted UNI Recv stream");
    });
    let stream_id = recv_stream.id().into_u64();

    let mut recv_stream = Arc::new(Mutex::new(recv_stream));
    recv_stream = Arc::clone(&recv_stream);

    tokio::spawn(
        async move {
            let stream = UniRecvStream::new(stable_id, stream_id, recv_stream);
            let senders = client.lock().await.senders();
            let mut uni_stream_receiver = UniStreamReceiver::init(stream, client).await;

            match uni_stream_receiver.start().instrument(session_span).await {
                Ok(_) => {}
                Err((code, reason)) => {
                    tracing::error!(reason);

                    let _ = senders
                        .close_session_tx()
                        .send((u8::from(code) as u64, reason.to_string()))
                        .await;
                }
            }

            let _ = uni_stream_receiver.finish().await;
        }
        .in_current_span(),
    );
    Ok(())
}

async fn spawn_uni_send_stream_thread(
    client: Arc<Mutex<MOQTClient>>,
    send_stream: SendStream,
    subscribe_id: u64,
    data_stream_type: DataStreamType,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!(
            "Open UNI Send stream for stream type: {:?}",
            data_stream_type
        );
    });
    let stream_id = send_stream.id().into_u64();

    tokio::spawn(
        async move {
            let stream = UniSendStream::new(stable_id, stream_id, subscribe_id, send_stream);
            let senders = client.lock().await.senders();

            let mut object_stream_forwarder =
                ObjectStreamForwarder::init(stream, client, data_stream_type)
                    .await
                    .unwrap();

            match object_stream_forwarder
                .start()
                .instrument(session_span)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    let code = TerminationErrorCode::InternalError;
                    let reason = format!("ObjectStreamForwarder: {:?}", e);

                    tracing::error!(reason);

                    let _ = senders
                        .close_session_tx()
                        .send((u8::from(code) as u64, reason.to_string()))
                        .await;
                }
            }

            let _ = object_stream_forwarder.terminate().await;
        }
        .in_current_span(),
    );
    Ok(())
}

async fn spawn_recv_datagram_thread(
    client: Arc<Mutex<MOQTClient>>,
    datagram: Datagram,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Accepted Datagram");
    });

    // No loop: End after receiving once
    tokio::spawn(
        async move {
            let senders = client.lock().await.senders();
            let mut datagram_receiver = DatagramReceiver::init(client).await;

            match datagram_receiver
                .receive(datagram)
                .instrument(session_span)
                .await
            {
                Ok(_) => {}
                Err((code, reason)) => {
                    tracing::error!(reason);

                    let _ = senders
                        .close_session_tx()
                        .send((u8::from(code) as u64, reason.to_string()))
                        .await;
                }
            }
        }
        .in_current_span(),
    );
    Ok(())
}

async fn spawn_send_datagram_thread(
    client: Arc<Mutex<MOQTClient>>,
    session: Arc<Connection>,
    subscribe_id: u64,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Accepted Datagram");
    });

    tokio::spawn(
        async move {
            let senders = client.lock().await.senders();
            let mut datagram_forwarder = DatagramForwarder::init(session, subscribe_id, client)
                .await
                .unwrap();

            match datagram_forwarder.start().instrument(session_span).await {
                Ok(_) => {}
                Err(e) => {
                    let code = TerminationErrorCode::InternalError;
                    let reason = format!("DatagramForwarder: {:?}", e);

                    tracing::error!(reason);

                    let _ = senders
                        .close_session_tx()
                        .send((u8::from(code) as u64, reason.to_string()))
                        .await;
                }
            }

            let _ = datagram_forwarder.finish().await;
        }
        .in_current_span(),
    );

    Ok(())
}

pub(crate) async fn select_spawn_thread(
    client: &Arc<Mutex<MOQTClient>>,
    session: Arc<Connection>,
    open_downstream_stream_or_datagram_rx: &mut mpsc::Receiver<(u64, DataStreamType)>,
    close_session_rx: &mut mpsc::Receiver<(u64, String)>,
    is_control_stream_opened: &mut bool,
) -> Result<()> {
    // TODO: FIXME: Need to store information between threads for QUIC-level reconnection support
    tokio::select! {
        stream = session.accept_bi() => {
            let (send_stream, recv_stream) = stream?;
            spawn_bi_stream_threads(client.clone(), send_stream, recv_stream, is_control_stream_opened).await?;
        },
        stream = session.accept_uni() => {
            let recv_stream = stream?;
            spawn_uni_recv_stream_thread(client.clone(), recv_stream).await?;
        },
        datagram = session.receive_datagram() => {
            let datagram = datagram?;
            spawn_recv_datagram_thread(client.clone(), datagram).await?;
        },
        // Waiting for a uni-directional send stream open request and forwarding the message
        Some((subscribe_id, data_stream_type)) = open_downstream_stream_or_datagram_rx.recv() => {
            match data_stream_type {
                DataStreamType::StreamHeaderTrack | DataStreamType::StreamHeaderSubgroup => {
                    let send_stream = session.open_uni().await?.await?;
                    spawn_uni_send_stream_thread(client.clone(), send_stream, subscribe_id, data_stream_type).await?;
                }
                DataStreamType::ObjectDatagram => {
                    let session = session.clone();
                    spawn_send_datagram_thread(client.clone(), session, subscribe_id).await?;

                }
            }
        },
        // TODO: Not implemented yet
        Some((code, reason)) = close_session_rx.recv() => {
            let msg = std::format!("Close session received (code: {:?}): {:?}", code, reason);
            tracing::error!(msg);
            // FIXME: I want to close the session, but VarInt is not exported, so I'll leave it as is
            // Maybe it's in wtransport-proto?
            // session.close(VarInt)
            bail!(msg);
        }
    }

    Ok(())
}

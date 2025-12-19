use super::{
    control_stream::{
        bi_stream::BiStream, handler::handle_control_stream, sender::send_control_stream,
    },
    data_streams::{
        datagram::{forwarder::DatagramObjectForwarder, receiver::DatagramObjectReceiver},
        subgroup_stream::{
            forwarder::{SubgroupForwarderError, SubgroupStreamObjectForwarder},
            receiver::SubgroupStreamObjectReceiver,
            uni_stream::{UniRecvStream, UniSendStream},
        },
    },
};
use crate::{
    SignalDispatchCommand, SubgroupStreamId,
    modules::{control_message_dispatcher::ControlMessageDispatchCommand, moqt_client::MOQTClient},
    signal_dispatcher::DataStreamThreadSignal,
};
use anyhow::{Result, bail};
use moqt_core::{
    constants::TerminationErrorCode, data_stream_type::DataStreamType,
    messages::moqt_payload::MOQTPayload,
};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task;
use tracing::{self, Instrument};
use wtransport::{Connection, RecvStream, SendStream, datagram::Datagram};

async fn spawn_control_stream_threads(
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
        tracing::info!("Accepted bi-directional stream");
    });

    let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
    senders
        .control_message_dispatch_tx()
        .send(ControlMessageDispatchCommand::Set {
            session_id: stable_id,
            sender: message_tx,
        })
        .await?;

    // The send_stream is wrapped with a Mutex to make it thread-safe since it can be called from multiple threads for returning and forwarding messages.
    let shared_send_stream = Arc::new(Mutex::new(send_stream));

    // Spawn a thread to listen for control messages from the client
    let send_stream = Arc::clone(&shared_send_stream);
    let session_span = session_span.clone();
    let stream_id = recv_stream.id().into_u64();
    task::Builder::new()
        .name(&format!(
            "ControlStreamReceiver-{}-{}",
            stable_id, stream_id
        ))
        .spawn(
            async move {
                let mut stream = BiStream::new(stable_id, stream_id, recv_stream, send_stream);
                handle_control_stream(&mut stream, client)
                    .instrument(session_span)
                    .await
            }
            .in_current_span(),
        )?;

    // Spawn a thread to send control messages: respond to the client or forward to the other client
    let send_stream = Arc::clone(&shared_send_stream);
    task::Builder::new()
        .name(&format!("ControlStreamSender-{}-{}", stable_id, stream_id))
        .spawn(
            async move {
                let session_span = tracing::info_span!("Session", stable_id);
                send_control_stream(send_stream, message_rx)
                    .instrument(session_span)
                    .await;
            }
            .in_current_span(),
        )?;

    Ok(())
}

async fn spawn_subgroup_stream_object_receiver_thread(
    client: Arc<Mutex<MOQTClient>>,
    recv_stream: RecvStream,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Accepted uni-directional recv stream");
    });
    let stream_id = recv_stream.id().into_u64();
    let (signal_tx, signal_rx) = mpsc::channel::<Box<DataStreamThreadSignal>>(1024);

    let senders = client.lock().await.senders();
    senders
        .signal_dispatch_tx()
        .send(SignalDispatchCommand::Set {
            session_id: stable_id,
            stream_id,
            sender: signal_tx,
        })
        .await
        .unwrap();

    task::Builder::new()
        .name(&format!("ObjectStreamReceiver-{}-{}", stable_id, stream_id))
        .spawn(
            async move {
                let stream = UniRecvStream::new(stable_id, stream_id, recv_stream);
                tracing::debug!("tokio::spawn stream_id: {}", stream_id);
                let senders = client.lock().await.senders();
                let mut stream_object_receiver =
                    SubgroupStreamObjectReceiver::init(stream, client, signal_rx)
                        .instrument(session_span.clone())
                        .await;

                match stream_object_receiver
                    .start()
                    .instrument(session_span.clone())
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

                let _ = stream_object_receiver
                    .finish()
                    .instrument(session_span)
                    .await;
            }
            .in_current_span(),
        )?;
    Ok(())
}

async fn spawn_subgroup_stream_object_forwarder_thread(
    client: Arc<Mutex<MOQTClient>>,
    send_stream: SendStream,
    subscribe_id: u64,
    subgroup_stream_id: SubgroupStreamId,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Open uni-directional send for subgroup stream",);
    });
    let stream_id = send_stream.id().into_u64();
    let (signal_tx, signal_rx) = mpsc::channel::<Box<DataStreamThreadSignal>>(1024);

    let senders = client.lock().await.senders();
    senders
        .signal_dispatch_tx()
        .send(SignalDispatchCommand::Set {
            session_id: stable_id,
            stream_id,
            sender: signal_tx,
        })
        .await
        .unwrap();

    task::Builder::new()
        .name(&format!(
            "ObjectStreamForwarder-client:{} stream:{} group:{} subgroup:{}",
            stable_id, stream_id, subgroup_stream_id.0, subgroup_stream_id.1
        ))
        .spawn(
            async move {
                let stream = UniSendStream::new(stable_id, stream_id, send_stream);
                let senders = client.lock().await.senders();

                let mut stream_object_forwarder = SubgroupStreamObjectForwarder::init(
                    stream,
                    subscribe_id,
                    client,
                    subgroup_stream_id,
                    signal_rx,
                )
                .instrument(session_span.clone())
                .await
                .unwrap();

                match stream_object_forwarder
                    .start()
                    .instrument(session_span.clone())
                    .await
                {
                    Ok(_) => {}
                    Err(SubgroupForwarderError::CacheMissing) => {
                        tracing::warn!(
                            "StreamObjectForwarder: Cache missing: finish forwarder worker and stream"
                        );
                    }
                    Err(SubgroupForwarderError::SendFailed(e)) => {
                        let code = TerminationErrorCode::InternalError;
                        let reason = format!("StreamObjectForwarder send failed: {:?}", e);

                        tracing::error!(reason);

                        let _ = senders
                            .close_session_tx()
                            .send((u8::from(code) as u64, reason.to_string()))
                            .await;
                    }
                    Err(SubgroupForwarderError::ForwardingPreferenceMismatch) => {
                        let code = TerminationErrorCode::InternalError;
                        let reason =
                            "StreamObjectForwarder forwarding preference mismatch".to_string();

                        tracing::error!(reason);

                        let _ = senders
                            .close_session_tx()
                            .send((u8::from(code) as u64, reason.clone()))
                            .await;
                    }
                    Err(SubgroupForwarderError::Other(err)) => {
                        let code = TerminationErrorCode::InternalError;
                        let reason = format!("StreamObjectForwarder: {:?}", err);

                        tracing::error!(reason);

                        let _ = senders
                            .close_session_tx()
                            .send((u8::from(code) as u64, reason.to_string()))
                            .await;
                    }
                }

                let _ = stream_object_forwarder
                    .finish()
                    .instrument(session_span)
                    .await;
            }
            .in_current_span(),
        )?;
    Ok(())
}

async fn spawn_datagram_object_receiver_thread(
    client: Arc<Mutex<MOQTClient>>,
    datagram: Datagram,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Received a datagram");
    });

    // No loop: End after receiving once
    task::Builder::new()
        .name(&format!("ObjectDatagramReceiver-{}", stable_id))
        .spawn(
            async move {
                let senders = client.lock().await.senders();
                let mut datagram_object_receiver = DatagramObjectReceiver::init(datagram, client)
                    .instrument(session_span.clone())
                    .await;

                match datagram_object_receiver
                    .start()
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
        )?;
    Ok(())
}

async fn spawn_datagram_object_forwarder_thread(
    client: Arc<Mutex<MOQTClient>>,
    session: Arc<Connection>,
    subscribe_id: u64,
) -> Result<()> {
    let stable_id = client.lock().await.id();
    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Open datagrams send thread");
    });

    task::Builder::new()
        .name(&format!("ObjectDatagramForwarder-{}", stable_id))
        .spawn(
            async move {
                let senders = client.lock().await.senders();
                let mut datagram_object_forwarder =
                    DatagramObjectForwarder::init(session, subscribe_id, client)
                        .instrument(session_span.clone())
                        .await
                        .unwrap();

                match datagram_object_forwarder
                    .start()
                    .instrument(session_span.clone())
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        let code = TerminationErrorCode::InternalError;
                        let reason = format!("DatagramObjectForwarder: {:?}", e);

                        tracing::error!(reason);

                        let _ = senders
                            .close_session_tx()
                            .send((u8::from(code) as u64, reason.to_string()))
                            .await;
                    }
                }

                let _ = datagram_object_forwarder
                    .finish()
                    .instrument(session_span)
                    .await;
            }
            .in_current_span(),
        )?;

    Ok(())
}

pub(crate) async fn select_spawn_thread(
    client: &Arc<Mutex<MOQTClient>>,
    session: Arc<Connection>,
    start_forwarder_rx: &mut mpsc::Receiver<(u64, DataStreamType, Option<SubgroupStreamId>)>,
    close_session_rx: &mut mpsc::Receiver<(u64, String)>,
    is_control_stream_opened: &mut bool, // TODO: separate it from arguments
) -> Result<()> {
    // TODO: FIXME: Need to store information between threads for QUIC-level reconnection support
    tokio::select! {
        stream = session.accept_bi() => {
            let (send_stream, recv_stream) = stream?;
            spawn_control_stream_threads(client.clone(), send_stream, recv_stream, is_control_stream_opened).await?;
        },
        stream = session.accept_uni() => {
            let recv_stream = stream?;
            spawn_subgroup_stream_object_receiver_thread(client.clone(), recv_stream).await?;
        },
        datagram = session.receive_datagram() => {
            let datagram = datagram?;
            spawn_datagram_object_receiver_thread(client.clone(), datagram).await?;
        },
        // Waiting for requests to open a new data stream thread
        Some((subscribe_id, data_stream_type, subgroup_stream_id)) = start_forwarder_rx.recv() => {
            match data_stream_type {
                DataStreamType::SubgroupHeader => {
                    let send_stream = session.open_uni().await?.await?;
                    let subgroup_stream_id = subgroup_stream_id.unwrap();
                    spawn_subgroup_stream_object_forwarder_thread(client.clone(), send_stream, subscribe_id, subgroup_stream_id).await?;
                }
                DataStreamType::ObjectDatagram | DataStreamType::ObjectDatagramStatus => {
                    let session = session.clone();
                    spawn_datagram_object_forwarder_thread(client.clone(), session, subscribe_id).await?;

                }
                DataStreamType::FetchHeader => {
                    unimplemented!();
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

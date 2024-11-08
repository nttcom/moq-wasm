use anyhow::{bail, Context, Result};
use bytes::BytesMut;
use std::{collections::HashMap, sync::Arc, thread, time::Duration};
use tokio::sync::{
    mpsc,
    mpsc::{Receiver, Sender},
    Mutex,
};
use tracing::{self, Instrument};
use wtransport::{
    endpoint::IncomingSession, Endpoint, Identity, RecvStream, SendStream, ServerConfig,
};
mod modules;
use modules::{
    buffer_manager::{buffer_manager, request_buffer, BufferCommand},
    control_message_handler::{control_message_handler, MessageProcessResult},
    logging,
    moqt_client::MOQTClient,
    object_cache_storage::{
        object_cache_storage, CacheHeader, CacheObject, ObjectCacheStorageCommand,
        ObjectCacheStorageWrapper,
    },
    object_stream_handler::{object_stream_handler, ObjectStreamProcessResult},
    pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    },
    send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    },
    stream_header_handler::{stream_header_handler, StreamHeaderProcessResult},
};
pub use moqt_core::constants;
use moqt_core::{
    constants::{StreamDirection, TerminationErrorCode, UnderlayType},
    control_message_type::ControlMessageType,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::{
            announce::Announce,
            announce_ok::AnnounceOk,
            subscribe::{FilterType, Subscribe},
            subscribe_error::SubscribeError,
            subscribe_namespace::SubscribeNamespace,
            subscribe_namespace_ok::SubscribeNamespaceOk,
            subscribe_ok::SubscribeOk,
        },
        data_streams::{
            stream_header_subgroup::StreamHeaderSubgroup, stream_header_track::StreamHeaderTrack,
            DataStreams,
        },
        moqt_payload::MOQTPayload,
    },
    models::tracks::ForwardingPreference,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::write_variable_integer,
};

type SubscribeId = u64;
type SenderToOpenSubscription = Sender<(SubscribeId, DataStreamType)>;

// Callback to validate the Auth parameter
pub enum AuthCallbackType {
    Announce,
    Subscribe,
}

pub type AuthCallbackFunctionType =
    fn(track_name: String, auth_payload: String, AuthCallbackType) -> Result<()>;

pub struct MOQTConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    pub underlay: UnderlayType,
    pub auth_callback: Option<AuthCallbackFunctionType>,
    pub log_level: String,
}

impl Default for MOQTConfig {
    fn default() -> Self {
        MOQTConfig::new()
    }
}

impl MOQTConfig {
    // TODO: use getter/setter
    pub fn new() -> MOQTConfig {
        MOQTConfig {
            port: 4433,
            cert_path: "./cert.pem".to_string(),
            key_path: "./key.pem".to_string(),
            keep_alive_interval_sec: 3,
            underlay: UnderlayType::Both,
            auth_callback: None,
            log_level: "DEBUG".to_string(),
        }
    }
}

pub struct MOQT {
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
    underlay: UnderlayType,
    // auth_callback: Option<AuthCallbackFunctionType>,
    log_level: String,
}

impl MOQT {
    pub fn new(config: MOQTConfig) -> MOQT {
        MOQT {
            port: config.port,
            cert_path: config.cert_path,
            key_path: config.key_path,
            keep_alive_interval_sec: config.keep_alive_interval_sec,
            underlay: config.underlay,
            // auth_callback: config.auth_callback,
            log_level: config.log_level,
        }
    }
    pub async fn start(&self) -> Result<()> {
        logging::init_logging(self.log_level.to_string());

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }
        let config = ServerConfig::builder()
            .with_bind_default(self.port)
            .with_identity(
                &Identity::load_pemfiles(&self.cert_path, &self.key_path)
                    .await
                    .with_context(|| {
                        format!(
                            "cert load failed. '{}' or '{}' not found.",
                            self.cert_path, self.key_path
                        )
                    })?,
            )
            .keep_alive_interval(Some(Duration::from_secs(self.keep_alive_interval_sec)))
            .build();
        let server = Endpoint::server(config)?;
        tracing::info!("Server ready!");

        // Spawn management thread
        let (buffer_tx, mut buffer_rx) = mpsc::channel::<BufferCommand>(1024);
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });
        let (pubsub_relation_tx, mut pubsub_relation_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut pubsub_relation_rx).await });
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);
        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let (object_cache_tx, mut object_cache_rx) =
            mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut object_cache_rx).await });

        let open_subscription_txes: HashMap<usize, SenderToOpenSubscription> = HashMap::new();
        let shared_open_subscription_txes = Arc::new(Mutex::new(open_subscription_txes));

        for id in 0.. {
            let incoming_session = server.accept().await;
            let connection_span = tracing::info_span!("Connection", id);

            let buffer_tx = buffer_tx.clone();
            let pubsub_relation_tx = pubsub_relation_tx.clone();
            let send_stream_tx = send_stream_tx.clone();
            let object_cache_tx = object_cache_tx.clone();
            let open_subscription_txes = shared_open_subscription_txes.clone();

            // Create a thread for each session
            tokio::spawn(async move {
                let result = handle_connection(
                    buffer_tx,
                    pubsub_relation_tx,
                    send_stream_tx,
                    object_cache_tx,
                    open_subscription_txes,
                    incoming_session,
                )
                .instrument(connection_span)
                .await;
                tracing::error!("{:?}", result);
            });
        }

        Ok(())
    }
}

async fn handle_connection(
    buffer_tx: mpsc::Sender<BufferCommand>,
    pubsub_relation_tx: mpsc::Sender<PubSubRelationCommand>,
    send_stream_tx: mpsc::Sender<SendStreamDispatchCommand>,
    object_cache_tx: mpsc::Sender<ObjectCacheStorageCommand>,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    incoming_session: IncomingSession,
) -> Result<()> {
    let session_request = incoming_session.await?;
    tracing::info!(
        "New session: Authority: '{}', Path: '{}'",
        session_request.authority(),
        session_request.path()
    );

    let connection = session_request.accept().await?;
    let stable_id = connection.stable_id();

    let client = Arc::new(Mutex::new(MOQTClient::new(stable_id)));

    let session_span = tracing::info_span!("Session", stable_id);
    session_span.in_scope(|| {
        tracing::info!("Waiting for data from client...");
    });

    // For opening a new data stream
    let (open_subscription_tx, mut open_subscription_rx) =
        mpsc::channel::<(SubscribeId, DataStreamType)>(32);
    open_subscription_txes
        .lock()
        .await
        .insert(stable_id, open_subscription_tx);

    let (close_connection_tx, mut close_connection_rx) = mpsc::channel::<(u64, String)>(32);

    // TODO: FIXME: Need to store information between threads for QUIC-level reconnection support
    let mut is_control_stream_opened = false;

    #[allow(unused_variables)]
    loop {
        tokio::select! {
            // Waiting for a bi-directional stream and processing the received message
            stream = connection.accept_bi() => {
                if is_control_stream_opened {
                    tracing::error!("Control stream already opened");
                    close_connection_tx.send((u8::from(TerminationErrorCode::ProtocolViolation) as u64, "Control stream already opened".to_string())).await?;
                    break;
                }
                is_control_stream_opened = true;

                let session_span = tracing::info_span!("Session", stable_id);
                session_span.in_scope(|| {
                    tracing::info!("Accepted BI stream");
                });

                let stream = stream?;

                // The send_stream is wrapped with a Mutex to make it thread-safe since it can be called from multiple threads for returning and relaying messages.
                let (send_stream, recv_stream) = stream;
                let shared_send_stream = Arc::new(Mutex::new(send_stream));

                let stream_id = recv_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let pubsub_relation_tx = pubsub_relation_tx.clone();
                let send_stream_tx = send_stream_tx.clone();
                let close_connection_tx = close_connection_tx.clone();
                let client= client.clone();

                let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
                send_stream_tx.send(SendStreamDispatchCommand::Set {
                    session_id: stable_id,
                    stream_direction: StreamDirection::Bi,
                    sender: message_tx,
                }).await?;

                // Spawn thread listenning for WebTransport messages
                let send_stream = Arc::clone(&shared_send_stream);
                let session_span_clone = session_span.clone();
                tokio::spawn(async move {
                    let mut stream = BiStream {
                        stable_id,
                        stream_id,
                        recv_stream,
                        shared_send_stream: send_stream,
                    };
                    handle_incoming_bi_stream(&mut stream, client, buffer_tx, pubsub_relation_tx, close_connection_tx, send_stream_tx).instrument(session_span_clone).await
                }.in_current_span());

                // Thread to relay messages (ANNOUNCE SUBSCRIBE) from the server
                let send_stream = Arc::clone(&shared_send_stream);
                tokio::spawn(async move {
                    let session_span = tracing::info_span!("Session", stable_id);
                    wait_and_relay_control_message(send_stream, message_rx).instrument(session_span).await;
                }.in_current_span());
            },
            // Waiting for a uni-directional recv stream and processing the received message
            stream = connection.accept_uni() => {
                let recv_stream = stream?;

                let session_span = tracing::info_span!("Session", stable_id);
                session_span.in_scope(|| {
                    tracing::info!("Accepted UNI Recv stream");
                });
                let stream_id = recv_stream.id().into_u64();

                let shared_recv_stream = Arc::new(Mutex::new(recv_stream));
                let shared_recv_stream = Arc::clone(&shared_recv_stream);

                let buffer_tx = buffer_tx.clone();
                let pubsub_relation_tx = pubsub_relation_tx.clone();
                let object_cache_tx = object_cache_tx.clone();
                let open_subscription_txes = open_subscription_txes.clone();
                let close_connection_tx = close_connection_tx.clone();
                let client = client.clone();
                let session_span_clone = session_span.clone();

                tokio::spawn(async move {
                    let mut stream = UniRecvStream {
                        stable_id,
                        stream_id,
                        shared_recv_stream,
                    };
                    handle_incoming_uni_stream(&mut stream, client, buffer_tx, pubsub_relation_tx, open_subscription_txes, close_connection_tx, object_cache_tx).instrument(session_span_clone).await
                }.in_current_span());

            },
            // Waiting for a uni-directional send stream open request and relay object
            Some((subscribe_id, data_stream_type)) = open_subscription_rx.recv() => {

                if !is_control_stream_opened {
                    // Decline the request if the control stream is not opened
                    tracing::error!("Control stream is not opened yet");
                    close_connection_tx.send((u8::from(TerminationErrorCode::ProtocolViolation) as u64, "Control stream already opened".to_string())).await?;
                    break;
                }

                match data_stream_type {
                    DataStreamType::StreamHeaderTrack | DataStreamType::StreamHeaderSubgroup => {
                        let session_span = tracing::info_span!("Session", stable_id);
                        session_span.in_scope(|| {
                            tracing::info!("Open UNI Send stream for stream type: {:?}", data_stream_type);
                        });

                        let buffer_tx = buffer_tx.clone();
                        let pubsub_relation_tx = pubsub_relation_tx.clone();
                        let object_cache_tx = object_cache_tx.clone();

                        let close_connection_tx = close_connection_tx.clone();
                        let session_span_clone = session_span.clone();
                        let send_stream = connection.open_uni().await?.await?;
                        let stream_id = send_stream.id().into_u64();

                        tokio::spawn(async move {
                            let mut stream = UniSendStream {
                                stable_id,
                                stream_id,
                                subscribe_id,
                                send_stream,
                            };
                            wait_and_relay_object_stream(&mut stream, data_stream_type, buffer_tx, pubsub_relation_tx, close_connection_tx, object_cache_tx).instrument(session_span_clone).await
                        }.in_current_span());


                    }
                    DataStreamType::ObjectDatagram => {
                        // TODO: Open datagram thread
                        unimplemented!();
                    }
                }


            },
            // TODO: Not implemented yet
            Some((_code, _reason)) = close_connection_rx.recv() => {
                tracing::error!("Close connection received");
                // FIXME: I want to close the connection, but VarInt is not exported, so I'll leave it as is
                // Maybe it's in wtransport-proto?
                // connection.close(VarInt)
                break;
            }
        }
    }

    // Delete pub/sub information related to the client
    let pubsub_relation_manager = PubSubRelationManagerWrapper::new(pubsub_relation_tx.clone());
    let _ = pubsub_relation_manager.delete_client(stable_id).await;

    // Delete object cache related to the client
    // FIXME: It should not be deleted if the cache should be stored
    //   (Now, it is deleted immediately because to clean up cpu and memory)
    let mut object_cache_storage = ObjectCacheStorageWrapper::new(object_cache_tx.clone());
    let _ = object_cache_storage.delete_client(stable_id).await;

    // Delete senders to the client
    send_stream_tx
        .send(SendStreamDispatchCommand::Delete {
            session_id: stable_id,
        })
        .await?;

    // FIXME: Do not remove if storing QUIC-level sessions
    buffer_tx
        .send(BufferCommand::ReleaseSession {
            session_id: stable_id,
        })
        .await?;

    tracing::info!("session terminated");

    Ok(())
}

struct UniRecvStream {
    stable_id: usize,
    stream_id: u64,
    shared_recv_stream: Arc<Mutex<RecvStream>>,
}

async fn handle_incoming_uni_stream(
    stream: &mut UniRecvStream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    pubsub_relation_tx: Sender<PubSubRelationCommand>,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    close_connection_tx: Sender<(u64, String)>,
    object_cache_tx: Sender<ObjectCacheStorageCommand>,
) -> Result<()> {
    let mut header_read = false;

    let stable_id = stream.stable_id;
    let mut upstream_subscribe_id: u64 = 0;
    let mut stream_header_type: DataStreamType = DataStreamType::ObjectDatagram;
    let stream_id = stream.stream_id;
    let mut client = client.lock().await;
    let close_connection_tx_clone = close_connection_tx.clone();
    let buffer_tx_clone = buffer_tx.clone();
    let shared_recv_stream = &mut stream.shared_recv_stream;
    let shared_recv_stream = Arc::clone(shared_recv_stream);

    let mut pubsub_relation_manager = PubSubRelationManagerWrapper::new(pubsub_relation_tx.clone());

    let mut object_cache_storage = ObjectCacheStorageWrapper::new(object_cache_tx.clone());

    let buf = request_buffer(buffer_tx_clone, stable_id, stream_id).await;
    let buf_clone = Arc::clone(&buf);

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
                        let _ = close_connection_tx_clone
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
                result = stream_header_handler(
                    &mut process_buf,
                    &mut client,
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
                        let open_subscription_tx = open_subscription_txes
                            .lock()
                            .await
                            .get(&downstream_session_id)
                            .unwrap()
                            .clone();

                        open_subscription_tx
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
                    close_connection_tx
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

            result = object_stream_handler(
                stream_header_type.clone(),
                upstream_subscribe_id,
                &mut process_buf,
                &mut client,
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
                close_connection_tx
                    .send((u8::from(code) as u64, message))
                    .await?;
                break;
            }
        }
    }

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    open_subscription_txes.lock().await.remove(&stable_id);

    Ok(())
}

struct UniSendStream {
    stable_id: usize,
    stream_id: u64,
    subscribe_id: u64,
    send_stream: SendStream,
}

async fn wait_and_relay_object_stream(
    stream: &mut UniSendStream,
    data_stream_type: DataStreamType,
    buffer_tx: Sender<BufferCommand>,
    pubsub_relation_tx: Sender<PubSubRelationCommand>,
    close_connection_tx: Sender<(u64, String)>,
    object_cache_tx: Sender<ObjectCacheStorageCommand>,
) -> Result<()> {
    let sleep_time = Duration::from_millis(10);

    let mut subgroup_header: Option<StreamHeaderSubgroup> = None; // For end group of AbsoluteRange

    let downstream_session_id = stream.stable_id;
    let downstream_stream_id = stream.stream_id;
    let downstream_subscribe_id = stream.subscribe_id;
    let send_stream = &mut stream.send_stream;

    let pubsub_relation_manager = PubSubRelationManagerWrapper::new(pubsub_relation_tx.clone());

    let mut object_cache_storage = ObjectCacheStorageWrapper::new(object_cache_tx.clone());

    // Get the information of the original publisher who has the track being requested
    let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
        .get_related_publisher(downstream_session_id, downstream_subscribe_id)
        .await?;
    let downstream_subscription = pubsub_relation_manager
        .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
        .await?
        .unwrap();
    let downstream_track_alias = downstream_subscription.get_track_alias();
    let filter_type = downstream_subscription.get_filter_type();
    let (start_group, start_object) = downstream_subscription.get_absolute_start();
    let (end_group, end_object) = downstream_subscription.get_absolute_end();

    // Validate the forwarding preference as Track
    match pubsub_relation_manager
        .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        Some(ForwardingPreference::Track) => {
            if data_stream_type != DataStreamType::StreamHeaderTrack {
                let msg = std::format!(
                    "uni send stream's data stream type is wrong (expected Track, but got {:?})",
                    data_stream_type
                );
                close_connection_tx
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.clone(),
                    ))
                    .await?;
                bail!(msg)
            }
            pubsub_relation_manager
                .set_downstream_forwarding_preference(
                    downstream_session_id,
                    downstream_subscribe_id,
                    ForwardingPreference::Track,
                )
                .await?;
        }
        Some(ForwardingPreference::Subgroup) => {
            if data_stream_type != DataStreamType::StreamHeaderSubgroup {
                let msg = std::format!(
                    "uni send stream's data stream type is wrong (expected Subgroup, but got {:?})",
                    data_stream_type
                );
                close_connection_tx
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.clone(),
                    ))
                    .await?;
                bail!(msg)
            }
            pubsub_relation_manager
                .set_downstream_forwarding_preference(
                    downstream_session_id,
                    downstream_subscribe_id,
                    ForwardingPreference::Subgroup,
                )
                .await?;
        }
        _ => {
            let msg = "Invalid forwarding preference";
            close_connection_tx
                .send((
                    u8::from(TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    };

    // Get the header from the cache storage and send it to the client
    match object_cache_storage
        .get_header(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        CacheHeader::Track(header) => {
            let mut buf = BytesMut::new();
            let header = StreamHeaderTrack::new(
                downstream_subscribe_id,
                downstream_track_alias,
                header.publisher_priority(),
            )
            .unwrap();

            header.packetize(&mut buf);

            let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
            message_buf.extend(write_variable_integer(
                u8::from(DataStreamType::StreamHeaderTrack) as u64,
            ));
            message_buf.extend(buf);

            if let Err(e) = send_stream.write_all(&message_buf).await {
                tracing::warn!("Failed to write to stream: {:?}", e);
                bail!(e);
            }
        }
        CacheHeader::Subgroup(header) => {
            let mut buf = BytesMut::new();
            let header = StreamHeaderSubgroup::new(
                downstream_subscribe_id,
                downstream_track_alias,
                header.group_id(),
                header.subgroup_id(),
                header.publisher_priority(),
            )
            .unwrap();

            subgroup_header = Some(header.clone());

            header.packetize(&mut buf);

            let mut message_buf = BytesMut::with_capacity(buf.len() + 8);
            message_buf.extend(write_variable_integer(
                u8::from(DataStreamType::StreamHeaderSubgroup) as u64,
            ));
            message_buf.extend(buf);

            if let Err(e) = send_stream.write_all(&message_buf).await {
                tracing::warn!("Failed to write to stream: {:?}", e);
                bail!(e);
            }
        }
        _ => {
            let msg = "cache header not matched";
            close_connection_tx
                .send((
                    u8::from(TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    }

    let mut object_cache_id: Option<usize> = None;

    while object_cache_id.is_none() {
        // Get the first object from the cache storage
        let result = match filter_type {
            FilterType::LatestGroup => {
                object_cache_storage
                    .get_latest_group(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::LatestObject => {
                object_cache_storage
                    .get_latest_object(upstream_session_id, upstream_subscribe_id)
                    .await
            }
            FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                object_cache_storage
                    .get_absolute_object(
                        upstream_session_id,
                        upstream_subscribe_id,
                        start_group.unwrap(),
                        start_object.unwrap(),
                    )
                    .await
            }
        };

        object_cache_id = match result {
            // Send the object to the client if the first cache is exist as track
            Ok(Some((id, CacheObject::Track(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                Some(id)
            }
            // Send the object to the client if the first cache is exist as subgroup
            Ok(Some((id, CacheObject::Subgroup(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                Some(id)
            }
            // Will be retried if the first cache is not exist
            Ok(None) => {
                thread::sleep(sleep_time);
                object_cache_id
            }
            _ => {
                let msg = "cache is not exist";
                close_connection_tx
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.to_string(),
                    ))
                    .await?;
                break;
            }
        };
    }

    loop {
        // Get the next object from the cache storage
        object_cache_id = match object_cache_storage
            .get_next_object(
                upstream_session_id,
                upstream_subscribe_id,
                object_cache_id.unwrap(),
            )
            .await
        {
            // Send the object to the client if the next cache is exist as track
            Ok(Some((id, CacheObject::Track(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                // Judge whether ids are reached to end of the range if the filter type is AbsoluteRange
                if filter_type == FilterType::AbsoluteRange {
                    let is_end = (object.group_id() == end_group.unwrap()
                        && object.object_id() == end_object.unwrap())
                        || (object.group_id() > end_group.unwrap());

                    if is_end {
                        break;
                    }
                }

                Some(id)
            }
            // Send the object to the client if the next cache is exist as subgroup
            Ok(Some((id, CacheObject::Subgroup(object)))) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);

                let mut message_buf = BytesMut::with_capacity(buf.len());
                message_buf.extend(buf);

                if let Err(e) = send_stream.write_all(&message_buf).await {
                    tracing::warn!("Failed to write to stream: {:?}", e);
                    bail!(e);
                }

                // Judge whether ids are reached to end of the range if the filter type is AbsoluteRange
                let header = subgroup_header.as_ref().unwrap();
                if filter_type == FilterType::AbsoluteRange {
                    let is_end = (header.group_id() == end_group.unwrap()
                        && object.object_id() == end_object.unwrap())
                        || (header.group_id() > end_group.unwrap());

                    if is_end {
                        break;
                    }
                }

                Some(id)
            }
            // Will be retried if the next cache is not exist
            Ok(None) => {
                thread::sleep(sleep_time);
                object_cache_id
            }
            _ => {
                let msg = "cache is not exist";
                close_connection_tx
                    .send((
                        u8::from(TerminationErrorCode::InternalError) as u64,
                        msg.to_string(),
                    ))
                    .await?;
                break;
            }
        };
    }

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: downstream_session_id,
            stream_id: downstream_stream_id,
        })
        .await?;

    Ok(())
}

struct BiStream {
    stable_id: usize,
    stream_id: u64,
    recv_stream: RecvStream,
    shared_send_stream: Arc<Mutex<SendStream>>,
}

async fn handle_incoming_bi_stream(
    stream: &mut BiStream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    pubsub_relation_tx: Sender<PubSubRelationCommand>,
    close_connection_tx: Sender<(u64, String)>,
    send_stream_tx: Sender<SendStreamDispatchCommand>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let recv_stream = &mut stream.recv_stream;
    let shared_send_stream = &mut stream.shared_send_stream;

    let mut pubsub_relation_manager = PubSubRelationManagerWrapper::new(pubsub_relation_tx.clone());
    let mut send_stream_dispatcher = SendStreamDispatcher::new(send_stream_tx.clone());

    loop {
        let bytes_read = match recv_stream.read(&mut buffer).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::debug!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = request_buffer(buffer_tx.clone(), stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf);

        let mut client = client.lock().await;
        // TODO: Move the implementation of control_message_handler to the server side since it is only used by the server
        let message_result = control_message_handler(
            &mut buf,
            UnderlayType::WebTransport,
            &mut client,
            &mut pubsub_relation_manager,
            &mut send_stream_dispatcher,
        )
        .await;

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
                close_connection_tx
                    .send((u8::from(code) as u64, message))
                    .await?;
                break;
            }
            MessageProcessResult::Fragment => (),
        };
    }

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    Ok(())
}

async fn wait_and_relay_control_message(
    send_stream: Arc<Mutex<SendStream>>,
    mut message_rx: Receiver<Arc<Box<dyn MOQTPayload>>>,
) {
    while let Some(message) = message_rx.recv().await {
        let mut write_buf = BytesMut::new();
        message.packetize(&mut write_buf);
        let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);

        if message.as_any().downcast_ref::<Subscribe>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Subscribe) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::Subscribe);
        } else if message.as_any().downcast_ref::<SubscribeOk>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeOk) as u64,
            ));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeOk
            );
        } else if message.as_any().downcast_ref::<SubscribeError>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeError) as u64,
            ));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeError
            );
        } else if message.as_any().downcast_ref::<Announce>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Announce) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::Announce);
        } else if message.as_any().downcast_ref::<AnnounceOk>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(ControlMessageType::AnnounceOk) as u64,
            ));
            tracing::info!("Relayed Message Type: {:?}", ControlMessageType::AnnounceOk);
        } else if message
            .as_any()
            .downcast_ref::<SubscribeNamespace>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeNamespace,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeNamespace
            );
        } else if message
            .as_any()
            .downcast_ref::<SubscribeNamespaceOk>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(u8::from(
                ControlMessageType::SubscribeNamespaceOk,
            ) as u64));
            tracing::info!(
                "Relayed Message Type: {:?}",
                ControlMessageType::SubscribeNamespaceOk
            );
        } else {
            tracing::warn!("Unsupported message type for bi-directional stream");
            continue;
        }
        message_buf.extend(write_variable_integer(write_buf.len() as u64));
        message_buf.extend(write_buf);

        let mut shared_send_stream = send_stream.lock().await;
        if let Err(e) = shared_send_stream.write_all(&message_buf).await {
            tracing::warn!("Failed to write to stream: {:?}", e);
            break;
        }

        tracing::info!("Control message is relayed.");
        // tracing::debug!("relayed message: {:?}", message_buf.to_vec());
    }
}

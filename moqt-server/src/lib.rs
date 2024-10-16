mod modules;
use crate::modules::object_stream_handler::object_stream_handler;
use crate::modules::stream_header_handler::StreamHeaderProcessResult;
use crate::modules::{
    buffer_manager,
    buffer_manager::{buffer_manager, BufferCommand},
    control_message_handler::*,
    pubsub_relation_manager::{commands::PubSubRelationCommand, manager::pubsub_relation_manager},
    send_stream_dispatcher::{send_stream_dispatcher, SendStreamDispatchCommand},
    stream_header_handler::stream_header_handler,
};
use anyhow::{bail, Context, Ok, Result};
use bytes::BytesMut;
use modules::object_cache_storage::{
    object_cache_storage, CacheHeader, CacheObject, ObjectCacheStorageCommand,
};
pub use moqt_core::constants;
use moqt_core::models::tracks::ForwardingPreference;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::{StreamDirection, UnderlayType},
    control_message_type::ControlMessageType,
    data_stream_type::DataStreamType,
    messages::{
        control_messages::{
            subscribe::Subscribe, subscribe_error::SubscribeError, subscribe_ok::SubscribeOk,
        },
        moqt_payload::MOQTPayload,
    },
    variable_integer::write_variable_integer,
    MOQTClient,
};
use std::time::Duration;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{
    mpsc,
    mpsc::{Receiver, Sender},
    Mutex,
};
use tracing::{self, Instrument};
use tracing_subscriber::{self, filter::LevelFilter, EnvFilter};
use wtransport::{
    endpoint::IncomingSession, Endpoint, Identity, RecvStream, SendStream, ServerConfig,
};

type SubscribeId = u64;
type SenderToOpenChannel = Sender<(SubscribeId, DataStreamType)>;

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
        init_logging(self.log_level.to_string());

        // For buffer management for each stream
        let (buffer_tx, mut buffer_rx) = mpsc::channel::<BufferCommand>(1024);
        // For track management
        let (pubsub_relation_tx, mut pubsub_relation_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        // For relay handler management
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);
        // For object cache
        let (object_cache_tx, mut object_cache_rx) =
            mpsc::channel::<ObjectCacheStorageCommand>(1024);

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }

        // Start buffer management thread
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });

        // Start track management thread
        tokio::spawn(async move { pubsub_relation_manager(&mut pubsub_relation_rx).await });

        // Start stream management thread
        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });

        // Start object cache thread
        tokio::spawn(async move { object_cache_storage(&mut object_cache_rx).await });

        let open_ch_txes: HashMap<usize, SenderToOpenChannel> = HashMap::new();
        let shared_open_ch_txes = Arc::new(Mutex::new(open_ch_txes));

        // Start wtransport server
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

        for id in 0.. {
            let buffer_tx = buffer_tx.clone();
            let pubsub_relation_tx = pubsub_relation_tx.clone();
            let send_stream_tx = send_stream_tx.clone();
            let object_cache_tx = object_cache_tx.clone();
            let open_ch_txes = shared_open_ch_txes.clone();
            let incoming_session = server.accept().await;
            let connection_span = tracing::info_span!("Connection", id);

            // Create a thread for each session
            tokio::spawn(async move {
                let result = handle_connection(
                    buffer_tx,
                    pubsub_relation_tx,
                    send_stream_tx,
                    object_cache_tx,
                    open_ch_txes,
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
    open_ch_txes: Arc<Mutex<HashMap<usize, SenderToOpenChannel>>>,
    incoming_session: IncomingSession,
) -> Result<()> {
    tracing::trace!("Waiting for session request...");

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
    let (open_ch_tx, mut open_ch_rx) = mpsc::channel::<(SubscribeId, DataStreamType)>(32);
    open_ch_txes.lock().await.insert(stable_id, open_ch_tx);

    let (close_conn_tx, mut close_conn_rx) = mpsc::channel::<(u64, String)>(32);

    // TODO: FIXME: Need to store information between threads for QUIC-level reconnection support
    let mut is_control_stream_opened = false;

    #[allow(unused_variables)]
    loop {
        tokio::select! {
            // Waiting for a bi-directional stream and processing the received message
            stream = connection.accept_bi() => {
                if is_control_stream_opened {
                    // Only 1 control stream is allowed
                    tracing::error!("Control stream already opened");
                    close_conn_tx.send((u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64, "Control stream already opened".to_string())).await?;
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
                let close_conn_tx = close_conn_tx.clone();
                let client= client.clone();

                let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
                send_stream_tx.send(SendStreamDispatchCommand::Set {
                    session_id: stable_id,
                    stream_direction: StreamDirection::Bi,
                    sender: message_tx,
                }).await?;

                // Thread that listens for WebTransport messages
                let send_stream = Arc::clone(&shared_send_stream);
                let session_span_clone = session_span.clone();
                tokio::spawn(async move {
                    let mut stream = BiStream {
                        stable_id,
                        stream_id,
                        recv_stream,
                        shared_send_stream: send_stream,
                    };
                    handle_incoming_bi_stream(&mut stream, client, buffer_tx, pubsub_relation_tx, close_conn_tx, send_stream_tx).instrument(session_span_clone).await

                // Propagate the current span (Connection)
                }.in_current_span());

                let send_stream = Arc::clone(&shared_send_stream);

                // Thread to relay messages (ANNOUNCE SUBSCRIBE) from the server
                tokio::spawn(async move {
                    let session_span = tracing::info_span!("Session", stable_id);
                    wait_and_relay_control_message(send_stream, message_rx).instrument(session_span).await;

                // Propagate the current span (Connection)
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

                let buffer_tx = buffer_tx.clone();
                let pubsub_relation_tx = pubsub_relation_tx.clone();
                let object_cache_tx = object_cache_tx.clone();
                let open_ch_txes = open_ch_txes.clone();
                let close_conn_tx = close_conn_tx.clone();
                let client = client.clone();
                let session_span_clone = session_span.clone();

                tokio::spawn(async move {
                    let mut stream = UniRecvStream {
                    stable_id,
                    stream_id,
                    recv_stream,
                };
                    handle_incoming_uni_stream(&mut stream, client, buffer_tx, pubsub_relation_tx, open_ch_txes, close_conn_tx, object_cache_tx).instrument(session_span_clone).await

                // Propagate the current span (Connection)
                }.in_current_span());

            },
            // Waiting for a uni-directional send stream open request and relaying the message
            Some((subscribe_id, data_stream_type)) = open_ch_rx.recv() => {

                if !is_control_stream_opened {
                    // Decline the request if the control stream is not opened
                    tracing::error!("Control stream is not opened yet");
                    close_conn_tx.send((u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64, "Control stream already opened".to_string())).await?;
                    break;
                }

                match data_stream_type {
                    DataStreamType::StreamHeaderTrack => {
                        let session_span = tracing::info_span!("Session", stable_id);
                        session_span.in_scope(|| {
                            tracing::info!("Open UNI Send stream");
                        });

                        let buffer_tx = buffer_tx.clone();
                        let pubsub_relation_tx = pubsub_relation_tx.clone();
                        let object_cache_tx = object_cache_tx.clone();

                        let close_conn_tx = close_conn_tx.clone();
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
                        relaying_track_stream(&mut stream, buffer_tx, pubsub_relation_tx, close_conn_tx, object_cache_tx).instrument(session_span_clone).await

                        // Propagate the current span (Connection)
                        }.in_current_span());


                    }
                    DataStreamType::StreamHeaderSubgroup => {
                        unimplemented!();
                    }
                    DataStreamType::ObjectDatagram => {
                        // TODO: Open datagram thread
                        unimplemented!();
                    }
                }


            },
            // TODO: Not implemented yet
            Some((_code, _reason)) = close_conn_rx.recv() => {
                tracing::error!("Close channel received");
                // FIXME: I want to close the connection, but VarInt is not exported, so I'll leave it as is
                // Maybe it's in wtransport-proto?
                // connection.close(VarInt)
                break;
            }
        }
    }

    // Delete pub/sub information related to the client
    let pubsub_relation_manager =
        modules::pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper::new(
            pubsub_relation_tx.clone(),
        );
    let _ = pubsub_relation_manager.delete_client(stable_id).await;

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
    recv_stream: RecvStream,
}

async fn handle_incoming_uni_stream(
    stream: &mut UniRecvStream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    pubsub_relation_tx: Sender<PubSubRelationCommand>,
    open_ch_txes: Arc<Mutex<HashMap<usize, SenderToOpenChannel>>>,
    close_conn_tx: Sender<(u64, String)>,
    object_cache_tx: Sender<ObjectCacheStorageCommand>,
) -> Result<()> {
    let mut header_read = false;
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id;
    let mut upstream_subscribe_id: u64 = 0;
    let mut stream_header_type: DataStreamType = DataStreamType::ObjectDatagram;
    let stream_id = stream.stream_id;
    let recv_stream = &mut stream.recv_stream;

    let mut pubsub_relation_manager =
        modules::pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper::new(
            pubsub_relation_tx.clone(),
        );

    let mut object_cache_storage =
        modules::object_cache_storage::ObjectCacheStorageWrapper::new(object_cache_tx.clone());

    loop {
        let bytes_read = match recv_stream.read(&mut buffer).await? {
            Some(bytes_read) => bytes_read,
            None => bail!("Failed to read from stream"),
        };

        tracing::debug!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = buffer_manager::request_buffer(buffer_tx.clone(), stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf);

        let mut client = client.lock().await;

        if !header_read {
            // Read header
            let result = stream_header_handler(
                &mut buf,
                &mut client,
                &mut pubsub_relation_manager,
                &mut object_cache_storage,
            )
            .await;

            match result {
                StreamHeaderProcessResult::Success((subscribe_id, header_type)) => {
                    upstream_subscribe_id = subscribe_id;
                    stream_header_type = header_type.clone();

                    // Open send uni-directional stream for subscribers
                    let subscribers = pubsub_relation_manager
                        .get_related_subscribers(stable_id, stream_id)
                        .await
                        .unwrap();

                    for (downstream_session_id, downstream_subscribe_id) in subscribers {
                        let open_ch_tx = open_ch_txes
                            .lock()
                            .await
                            .get(&downstream_session_id)
                            .unwrap()
                            .clone();

                        open_ch_tx
                            .send((downstream_subscribe_id, stream_header_type.clone()))
                            .await?;
                    }

                    header_read = true;
                }
                StreamHeaderProcessResult::Failure(code, message) => {
                    close_conn_tx.send((u8::from(code) as u64, message)).await?;
                    break;
                }
            }
        } else {
            // Read Object Stream
            object_stream_handler(
                stream_header_type.clone(),
                upstream_subscribe_id,
                &mut buf,
                &mut client,
                &mut object_cache_storage,
            )
            .await;
        }
    }

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    open_ch_txes.lock().await.remove(&stable_id);

    Ok(())
}

struct UniSendStream {
    stable_id: usize,
    stream_id: u64,
    subscribe_id: u64,
    send_stream: SendStream,
}

async fn relaying_track_stream(
    stream: &mut UniSendStream,
    buffer_tx: Sender<BufferCommand>,
    pubsub_relation_tx: Sender<PubSubRelationCommand>,
    close_conn_tx: Sender<(u64, String)>,
    object_cache_tx: Sender<ObjectCacheStorageCommand>,
) -> Result<()> {
    let downstream_session_id = stream.stable_id;
    let downstream_stream_id = stream.stream_id;
    let downstream_subscribe_id = stream.subscribe_id;
    let send_stream = &mut stream.send_stream;

    let pubsub_relation_manager =
        modules::pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper::new(
            pubsub_relation_tx.clone(),
        );

    let mut object_cache_storage =
        modules::object_cache_storage::ObjectCacheStorageWrapper::new(object_cache_tx.clone());

    // Get the information of the original publisher who has the track or subgroup being requested
    let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager
        .get_related_publisher(downstream_session_id, downstream_subscribe_id)
        .await?;
    let downstream_subscription = pubsub_relation_manager
        .get_downstream_subscription_by_ids(downstream_session_id, downstream_subscribe_id)
        .await?
        .unwrap();
    let downstream_track_alias = downstream_subscription.get_track_alias();

    match pubsub_relation_manager
        .get_upstream_forwarding_preference(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        Some(ForwardingPreference::Track) => {
            pubsub_relation_manager
                .set_downstream_forwarding_preference(
                    downstream_session_id,
                    downstream_subscribe_id,
                    ForwardingPreference::Track,
                )
                .await?;
            ForwardingPreference::Track
        }
        _ => {
            let msg = "Invalid forwarding preference";
            close_conn_tx
                .send((
                    u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64,
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
            let mut header = header.clone();

            header.set_subscribe_id(downstream_subscribe_id);
            header.set_track_alias(downstream_track_alias);
            header.packetize(&mut buf);

            send_stream.write_all(&buf).await?;
        }
        _ => {
            let msg = "cache header not matched";
            close_conn_tx
                .send((
                    u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    }

    // TODO: Change the method of obtaining objects according to the subscribe request
    // Get the first object from the cache storage and send it to the client
    let mut object_cache_id = match object_cache_storage
        .get_first_object(upstream_session_id, upstream_subscribe_id)
        .await?
    {
        (id, CacheObject::Track(object)) => {
            let mut buf = BytesMut::new();
            object.packetize(&mut buf);
            send_stream.write_all(&buf).await?;

            id
        }
        _ => {
            let msg = "cache object not matched";
            close_conn_tx
                .send((
                    u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64,
                    msg.to_string(),
                ))
                .await?;
            bail!(msg)
        }
    };

    loop {
        // TODO: Implement the processing when the content ends
        // Get the first object from the cache storage and send it to the client
        object_cache_id = match object_cache_storage
            .get_next_object(upstream_session_id, upstream_subscribe_id, object_cache_id)
            .await?
        {
            (id, CacheObject::Track(object)) => {
                let mut buf = BytesMut::new();
                object.packetize(&mut buf);
                send_stream.write_all(&buf).await?;

                id
            }
            _ => {
                let msg = "cache object not matched";
                close_conn_tx
                    .send((
                        u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64,
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
    close_conn_tx: Sender<(u64, String)>,
    send_stream_tx: Sender<SendStreamDispatchCommand>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let recv_stream = &mut stream.recv_stream;
    let shared_send_stream = &mut stream.shared_send_stream;

    let mut pubsub_relation_manager =
        modules::pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper::new(
            pubsub_relation_tx.clone(),
        );
    let mut send_stream_dispatcher =
        modules::send_stream_dispatcher::SendStreamDispatcher::new(send_stream_tx.clone());

    loop {
        let bytes_read = match recv_stream.read(&mut buffer).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::debug!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = buffer_manager::request_buffer(buffer_tx.clone(), stable_id, stream_id).await;
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
                close_conn_tx.send((u8::from(code) as u64, message)).await?;
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
        tracing::debug!("relayed message: {:?}", message_buf.to_vec());
    }
}

fn init_logging(log_level: String) {
    let level_filter: LevelFilter = match log_level.to_uppercase().as_str() {
        "OFF" => LevelFilter::OFF,
        "TRACE" => LevelFilter::TRACE,
        "DEBUG" => LevelFilter::DEBUG,
        "INFO" => LevelFilter::INFO,
        "WARN" => LevelFilter::WARN,
        "ERROR" => LevelFilter::ERROR,
        _ => {
            panic!(
                "Invalid log level: '{}'.\n  Valid log levels: [OFF, TRACE, DEBUG, INFO, WARN, ERROR]",
                log_level
            );
        }
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(level_filter.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();

    tracing::info!("Logging initialized. (Level: {})", log_level);
}

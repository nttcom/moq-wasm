use crate::constants::TerminationErrorCode;
use crate::modules::object_cache_storage::CacheHeader;
use crate::modules::object_cache_storage::CacheObject;
use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;
use crate::SenderToOpenSubscription;
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::messages::data_streams::object_datagram::ObjectDatagram;
use moqt_core::messages::data_streams::DataStreams;
use moqt_core::models::tracks::ForwardingPreference;
use moqt_core::moqt_client::MOQTClientStatus;
use moqt_core::variable_integer::read_variable_integer;
use moqt_core::PubSubRelationManagerRepository;
use moqt_core::{data_stream_type::DataStreamType, MOQTClient};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum ObjectDatagramProcessResult {
    Success,
    Continue,
    Failure(TerminationErrorCode, String),
}

fn read_header_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<DataStreamType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let header_type: DataStreamType = match DataStreamType::try_from(type_value) {
        Ok(v) => {
            if v == DataStreamType::StreamHeaderTrack || v == DataStreamType::StreamHeaderSubgroup {
                bail!("{:?} is not header type", v);
            }
            v
        }
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(header_type)
}

pub async fn object_datagram_handler(
    read_buf: &mut BytesMut,
    client: Arc<Mutex<MOQTClient>>,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
    open_subscription_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
) -> ObjectDatagramProcessResult {
    let payload_length = read_buf.len();
    tracing::trace!("object_datagram_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return ObjectDatagramProcessResult::Continue;
    }

    // TODO: Set the accurate duration
    let duration = 100000;

    let mut read_cur = Cursor::new(&read_buf[..]);

    // Read the header type
    let header_type = match read_header_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("header_type is wrong: {:?}", err);
            return ObjectDatagramProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };

    let client_status: MOQTClientStatus;
    let upstream_session_id: usize;
    {
        let client = client.lock().await;
        client_status = client.status();
        upstream_session_id = client.id;
    }
    // check subscription and judge if it is invalid timing
    if client_status != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return ObjectDatagramProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            message,
        );
    }

    tracing::debug!("object_stream: read_buf: {:?}", read_buf);

    match header_type {
        DataStreamType::ObjectDatagram => {
            let result = ObjectDatagram::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    read_buf.advance(read_cur.position() as usize);

                    let upstream_subscribe_id = object.subscribe_id();

                    match object_cache_storage
                        .get_header(upstream_session_id, upstream_subscribe_id)
                        .await
                    {
                        Ok(CacheHeader::Datagram) => {}
                        // It's first time to receive datagram
                        Err(_) => {
                            let _ = pubsub_relation_manager_repository
                                .set_upstream_forwarding_preference(
                                    upstream_session_id,
                                    upstream_subscribe_id,
                                    ForwardingPreference::Datagram,
                                )
                                .await;

                            let _ = object_cache_storage
                                .set_subscription(
                                    upstream_session_id,
                                    upstream_subscribe_id,
                                    CacheHeader::Datagram,
                                )
                                .await;

                            // Open send uni-directional datagram for subscribers
                            let subscribers = pubsub_relation_manager_repository
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
                                    .send((downstream_subscribe_id, header_type.clone()))
                                    .await;
                            }
                        }
                        _ => {
                            let msg = "failed to get cache header, error: unexpected cache header is already set"
                .to_string();
                            tracing::error!(msg);
                            return ObjectDatagramProcessResult::Failure(
                                TerminationErrorCode::InternalError,
                                msg,
                            );
                        }
                    }

                    let cache_object = CacheObject::Datagram(object);
                    object_cache_storage
                        .set_object(
                            upstream_session_id,
                            upstream_subscribe_id,
                            cache_object,
                            duration,
                        )
                        .await
                        .unwrap();
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    read_cur.set_position(0);
                    return ObjectDatagramProcessResult::Continue;
                }
            }
        }
        _ => {
            return ObjectDatagramProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Invalid message type: {:?}", header_type),
            );
        }
    };

    ObjectDatagramProcessResult::Success
}

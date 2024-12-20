use crate::constants::TerminationErrorCode;
use crate::modules::{
    moqt_client::{MOQTClient, MOQTClientStatus},
    object_cache_storage::{CacheHeader, CacheObject, ObjectCacheStorageWrapper},
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{object_datagram::ObjectDatagram, DataStreams},
    models::tracks::ForwardingPreference,
    variable_integer::read_variable_integer,
    PubSubRelationManagerRepository,
};
use std::{io::Cursor, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum ObjectDatagramProcessResult {
    Success((CacheObject, bool)),
    IncompleteMessage,
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
) -> ObjectDatagramProcessResult {
    let payload_length = read_buf.len();
    tracing::trace!("object_datagram_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return ObjectDatagramProcessResult::IncompleteMessage;
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
        upstream_session_id = client.id();
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

                    let is_first_time = match object_cache_storage
                        .get_header(upstream_session_id, upstream_subscribe_id)
                        .await
                    {
                        Ok(CacheHeader::Datagram) => {
                            // It's not first time to receive datagram
                            false
                        }
                        Err(_) => {
                            // It's first time to receive datagram
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

                            true
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
                    };

                    let received_object = CacheObject::Datagram(object);
                    object_cache_storage
                        .set_object(
                            upstream_session_id,
                            upstream_subscribe_id,
                            received_object.clone(),
                            duration,
                        )
                        .await
                        .unwrap();

                    ObjectDatagramProcessResult::Success((received_object, is_first_time))
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    read_cur.set_position(0);
                    ObjectDatagramProcessResult::IncompleteMessage
                }
            }
        }
        _ => ObjectDatagramProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            format!("Invalid message type: {:?}", header_type),
        ),
    }
}

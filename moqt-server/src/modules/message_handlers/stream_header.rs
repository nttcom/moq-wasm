pub(crate) mod handlers;
pub(crate) mod server_processes;

use crate::{
    constants::TerminationErrorCode,
    modules::{
        message_handlers::stream_header::server_processes::{
            stream_track_header::process_stream_header_track,
            stream_track_subgroup::process_stream_header_subgroup,
        },
        moqt_client::{MOQTClient, MOQTClientStatus},
        object_cache_storage::{CacheHeader, ObjectCacheStorageWrapper},
    },
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::read_variable_integer,
};
use std::io::Cursor;

#[derive(Debug, PartialEq)]
pub enum StreamHeaderProcessResult {
    Success(CacheHeader),
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
            if v == DataStreamType::ObjectDatagram {
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

pub async fn stream_header_handler(
    read_buf: &mut BytesMut,
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> StreamHeaderProcessResult {
    let payload_length = read_buf.len();
    tracing::trace!("stream_header_handler! {}", payload_length);

    // Check if the header type is exist
    if payload_length == 0 {
        return StreamHeaderProcessResult::IncompleteMessage;
    }

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::debug!("read_cur! {:?}", read_cur);

    // Read the header type
    let header_type = match read_header_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("header_type is wrong: {:?}", err);
            return StreamHeaderProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received Header Type: {:?}", header_type);

    // check subscription and judge if it is invalid timing
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return StreamHeaderProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            message,
        );
    }

    match header_type {
        DataStreamType::StreamHeaderTrack => {
            match process_stream_header_track(
                &mut read_cur,
                pubsub_relation_manager_repository,
                object_cache_storage,
                client,
            )
            .await
            {
                Ok(received_header) => {
                    read_buf.advance(read_cur.position() as usize);

                    StreamHeaderProcessResult::Success(received_header)
                }
                Err(err) => {
                    read_buf.advance(read_cur.position() as usize);
                    StreamHeaderProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    )
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            match process_stream_header_subgroup(
                &mut read_cur,
                pubsub_relation_manager_repository,
                object_cache_storage,
                client,
            )
            .await
            {
                Ok(received_header) => {
                    read_buf.advance(read_cur.position() as usize);

                    StreamHeaderProcessResult::Success(received_header)
                }
                Err(err) => {
                    read_buf.advance(read_cur.position() as usize);
                    StreamHeaderProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    )
                }
            }
        }
        unknown => StreamHeaderProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            format!("Unknown message type: {:?}", unknown),
        ),
    }
}

use crate::constants::TerminationErrorCode;
use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;
use crate::modules::server_processes::stream_track_header::process_stream_header_track;
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::moqt_client::MOQTClientStatus;
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    data_stream_type::DataStreamType, variable_integer::read_variable_integer, MOQTClient,
};
use std::io::Cursor;

#[derive(Debug, PartialEq)]
pub enum StreamHeaderProcessResult {
    Success((u64, DataStreamType)),
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
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> StreamHeaderProcessResult {
    let payload_length = read_buf.len();
    tracing::trace!("stream_header_handler! {}", payload_length);

    // Check if the header type is exist
    if payload_length == 0 {
        return StreamHeaderProcessResult::Continue;
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

    let subscribe_id = match header_type {
        DataStreamType::StreamHeaderTrack => {
            match process_stream_header_track(
                &mut read_cur,
                pubsub_relation_manager_repository,
                object_cache_storage,
                client,
            )
            .await
            {
                Ok(subscribe_id) => {
                    read_buf.advance(read_cur.position() as usize);
                    subscribe_id
                }
                Err(err) => {
                    read_buf.advance(read_cur.position() as usize);
                    return StreamHeaderProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            unimplemented!();
        }
        unknown => {
            return StreamHeaderProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    StreamHeaderProcessResult::Success((subscribe_id, header_type))
}

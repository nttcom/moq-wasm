use bytes::{Buf, BytesMut};
use std::io::Cursor;

use crate::{
    constants::TerminationErrorCode,
    modules::{
        moqt_client::{MOQTClient, MOQTClientStatus},
        object_cache_storage::{CacheObject, ObjectCacheStorageWrapper},
    },
};

use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{
        object_stream_subgroup::ObjectStreamSubgroup, object_stream_track::ObjectStreamTrack,
        DataStreams,
    },
};

#[derive(Debug, PartialEq)]
pub enum ObjectStreamProcessResult {
    Success,
    Continue,
    Failure(TerminationErrorCode, String),
}

pub async fn object_stream_handler(
    header_type: DataStreamType,
    subscribe_id: u64,
    read_buf: &mut BytesMut,
    client: &mut MOQTClient,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> ObjectStreamProcessResult {
    let payload_length = read_buf.len();
    tracing::trace!("object_stream_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return ObjectStreamProcessResult::Continue;
    }

    // TODO: Set the accurate duration
    let duration = 100000;

    let mut read_cur = Cursor::new(&read_buf[..]);

    // check subscription and judge if it is invalid timing
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return ObjectStreamProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            message,
        );
    }

    tracing::debug!("object_stream: read_buf: {:?}", read_buf);

    match header_type {
        DataStreamType::StreamHeaderTrack => {
            let result = ObjectStreamTrack::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    read_buf.advance(read_cur.position() as usize);

                    let cache_object = CacheObject::Track(object);
                    object_cache_storage
                        .set_object(client.id(), subscribe_id, cache_object, duration)
                        .await
                        .unwrap();
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    read_cur.set_position(0);
                    return ObjectStreamProcessResult::Continue;
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            let result = ObjectStreamSubgroup::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    read_buf.advance(read_cur.position() as usize);

                    let cache_object = CacheObject::Subgroup(object);
                    object_cache_storage
                        .set_object(client.id(), subscribe_id, cache_object, duration)
                        .await
                        .unwrap();
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    read_cur.set_position(0);
                    return ObjectStreamProcessResult::Continue;
                }
            }
        }
        unknown => {
            return ObjectStreamProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    ObjectStreamProcessResult::Success
}

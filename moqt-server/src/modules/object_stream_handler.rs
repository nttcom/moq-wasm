use crate::constants::TerminationErrorCode;
use crate::modules::object_cache_storage::CacheObject;
use crate::modules::object_cache_storage::ObjectCacheStorageWrapper;
use bytes::BytesMut;
use moqt_core::messages::data_streams::object_stream_track::ObjectStreamTrack;
use moqt_core::messages::moqt_payload::MOQTPayload;
use moqt_core::moqt_client::MOQTClientStatus;
use moqt_core::{data_stream_type::DataStreamType, MOQTClient};

#[derive(Debug, PartialEq)]
pub enum ObjectStreamProcessResult {
    Success,
    Failure(TerminationErrorCode, String),
}

pub async fn object_stream_handler(
    header_type: DataStreamType,
    subscribe_id: u64,
    read_buf: &mut BytesMut,
    client: &mut MOQTClient,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> ObjectStreamProcessResult {
    // TODO: Set the accurate duration
    let duration = 10000;
    tracing::trace!("object_stream_handler! {}", read_buf.len());

    // check subscription and judge if it is invalid timing
    if client.status() != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return ObjectStreamProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            message,
        );
    }

    match header_type {
        DataStreamType::StreamHeaderTrack => {
            let result = ObjectStreamTrack::depacketize(read_buf);
            match result {
                Ok(object) => {
                    let cache_object = CacheObject::Track(object);
                    object_cache_storage
                        .set_object(client.id, subscribe_id, cache_object, duration)
                        .await
                        .unwrap();
                }
                Err(err) => {
                    return ObjectStreamProcessResult::Failure(
                        TerminationErrorCode::ProtocolViolation,
                        err.to_string(),
                    );
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            unimplemented!();
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

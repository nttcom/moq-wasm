use crate::constants::TerminationErrorCode;
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{
        object_stream_subgroup::ObjectStreamSubgroup, object_stream_track::ObjectStreamTrack,
        DataStreams,
    },
};
use std::io::Cursor;

#[derive(Debug, PartialEq)]
pub enum ObjectStreamProcessResult {
    Success(StreamObject),
    Continue,
    Failure(TerminationErrorCode, String),
}

#[derive(Debug, PartialEq)]
pub enum StreamObject {
    Track(ObjectStreamTrack),
    Subgroup(ObjectStreamSubgroup),
}

pub async fn try_read_object(
    buf: &mut BytesMut,
    data_stream_type: DataStreamType,
) -> ObjectStreamProcessResult {
    let payload_length = buf.len();
    tracing::trace!("object_stream_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return ObjectStreamProcessResult::Continue;
    }

    let mut read_cur = Cursor::new(&buf[..]);

    match data_stream_type {
        DataStreamType::StreamHeaderTrack => {
            let result = ObjectStreamTrack::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    buf.advance(read_cur.position() as usize);

                    let object = StreamObject::Track(object);
                    ObjectStreamProcessResult::Success(object)
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    // Reset the cursor position because data for an object has not yet arrived
                    read_cur.set_position(0);

                    ObjectStreamProcessResult::Continue
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            let result = ObjectStreamSubgroup::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    buf.advance(read_cur.position() as usize);

                    let object = StreamObject::Subgroup(object);
                    ObjectStreamProcessResult::Success(object)
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    // // Reset the cursor position because data for an object has not yet arrived
                    read_cur.set_position(0);

                    ObjectStreamProcessResult::Continue
                }
            }
        }
        unknown => ObjectStreamProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            format!("Unknown message type: {:?}", unknown),
        ),
    }
}

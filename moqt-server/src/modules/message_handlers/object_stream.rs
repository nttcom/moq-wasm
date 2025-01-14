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

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::message_handlers::object_stream::{
            try_read_object, ObjectStreamProcessResult, StreamObject,
        };
        use bytes::BytesMut;
        use moqt_core::{
            data_stream_type::DataStreamType,
            messages::data_streams::{
                object_stream_subgroup::ObjectStreamSubgroup,
                object_stream_track::ObjectStreamTrack, DataStreams,
            },
        };
        use std::io::Cursor;

        #[tokio::test]
        async fn stream_object_track_success() {
            let data_stream_type = DataStreamType::StreamHeaderTrack;
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let buf_clone = buf.clone();

            let result = try_read_object(&mut buf, data_stream_type).await;

            let mut read_cur = Cursor::new(&buf_clone[..]);
            let object = ObjectStreamTrack::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                ObjectStreamProcessResult::Success(StreamObject::Track(object))
            );
        }

        #[tokio::test]
        async fn stream_object_subgroup_success() {
            let data_stream_type = DataStreamType::StreamHeaderSubgroup;
            let bytes_array = [
                0, // Object ID (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let buf_clone = buf.clone();

            let result = try_read_object(&mut buf, data_stream_type).await;

            let mut read_cur = Cursor::new(&buf_clone[..]);
            let object = ObjectStreamSubgroup::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                ObjectStreamProcessResult::Success(StreamObject::Subgroup(object))
            );
        }

        #[tokio::test]
        async fn stream_object_track_continue_insufficient_payload() {
            let data_stream_type = DataStreamType::StreamHeaderTrack;
            let bytes_array = [
                0,  // Group ID (i)
                1,  // Object ID (i)
                50, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = try_read_object(&mut buf, data_stream_type).await;

            assert_eq!(result, ObjectStreamProcessResult::Continue);
        }

        #[tokio::test]
        async fn stream_object_subgroup_continue_insufficient_payload() {
            let data_stream_type = DataStreamType::StreamHeaderSubgroup;
            let bytes_array = [
                0,  // Object ID (i)
                50, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = try_read_object(&mut buf, data_stream_type).await;

            assert_eq!(result, ObjectStreamProcessResult::Continue);
        }

        #[tokio::test]
        async fn stream_object_track_continue_incomplete_message() {
            let data_stream_type = DataStreamType::StreamHeaderTrack;
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = try_read_object(&mut buf, data_stream_type).await;

            assert_eq!(result, ObjectStreamProcessResult::Continue);
        }

        #[tokio::test]
        async fn stream_object_subgroup_continue_incomplete_message() {
            let data_stream_type = DataStreamType::StreamHeaderSubgroup;
            let bytes_array = [
                0, // Object ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = try_read_object(&mut buf, data_stream_type).await;

            assert_eq!(result, ObjectStreamProcessResult::Continue);
        }
    }
}

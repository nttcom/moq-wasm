use crate::constants::TerminationErrorCode;
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{subgroup_stream, track_stream, DataStreams},
};
use std::io::Cursor;

#[derive(Debug, PartialEq)]
pub enum StreamObjectProcessResult {
    Success(StreamObject),
    Continue,
    Failure(TerminationErrorCode, String),
}

#[derive(Debug, PartialEq)]
pub enum StreamObject {
    Track(track_stream::Object),
    Subgroup(subgroup_stream::Object),
}

pub async fn try_read_object(
    buf: &mut BytesMut,
    data_stream_type: DataStreamType,
) -> StreamObjectProcessResult {
    let payload_length = buf.len();
    tracing::trace!("stream_object_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return StreamObjectProcessResult::Continue;
    }

    let mut read_cur = Cursor::new(&buf[..]);
    let result = match data_stream_type {
        DataStreamType::StreamHeaderTrack => {
            track_stream::Object::depacketize(&mut read_cur).map(StreamObject::Track)
        }
        DataStreamType::StreamHeaderSubgroup => {
            subgroup_stream::Object::depacketize(&mut read_cur).map(StreamObject::Subgroup)
        }
        unknown => {
            return StreamObjectProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    match result {
        Ok(stream_object) => {
            buf.advance(read_cur.position() as usize);
            StreamObjectProcessResult::Success(stream_object)
        }
        Err(err) => {
            tracing::warn!("{:#?}", err);
            // Reset the cursor position because data for an object has not yet arrived
            read_cur.set_position(0);
            StreamObjectProcessResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::message_handlers::stream_object::{
            try_read_object, StreamObject, StreamObjectProcessResult,
        };
        use bytes::BytesMut;
        use moqt_core::{
            data_stream_type::DataStreamType,
            messages::data_streams::{subgroup_stream, track_stream, DataStreams},
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
            let object = track_stream::Object::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                StreamObjectProcessResult::Success(StreamObject::Track(object))
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
            let object = subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                StreamObjectProcessResult::Success(StreamObject::Subgroup(object))
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

            assert_eq!(result, StreamObjectProcessResult::Continue);
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

            assert_eq!(result, StreamObjectProcessResult::Continue);
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

            assert_eq!(result, StreamObjectProcessResult::Continue);
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

            assert_eq!(result, StreamObjectProcessResult::Continue);
        }
    }
}

use bytes::{Buf, BytesMut};
use moqt_core::messages::data_streams::{subgroup_stream, DataStreams};
use std::io::Cursor;

#[derive(Debug, PartialEq)]
pub enum SubgroupStreamObjectProcessResult {
    Success(subgroup_stream::Object),
    Continue,
}

pub async fn read_object(buf: &mut BytesMut) -> SubgroupStreamObjectProcessResult {
    let payload_length = buf.len();
    tracing::trace!("stream_object_handler! {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return SubgroupStreamObjectProcessResult::Continue;
    }

    let mut read_cur = Cursor::new(&buf[..]);

    match subgroup_stream::Object::depacketize(&mut read_cur) {
        Ok(stream_object) => {
            buf.advance(read_cur.position() as usize);
            SubgroupStreamObjectProcessResult::Success(stream_object)
        }
        Err(_err) => {
            // TODO: `buffer does not have enough length` is not error. we want to change it to `Continue`
            // tracing::info!("{:#?}", err);
            // Reset the cursor position because data for an object has not yet arrived
            read_cur.set_position(0);
            SubgroupStreamObjectProcessResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::message_handlers::subgroup_stream_object::{
            read_object, SubgroupStreamObjectProcessResult,
        };
        use bytes::BytesMut;
        use moqt_core::messages::data_streams::{subgroup_stream, DataStreams};
        use std::io::Cursor;

        #[tokio::test]
        async fn stream_object_subgroup_success() {
            let bytes_array = [
                0, // Object ID (i)
                0, // Extension Header Length (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let buf_clone = buf.clone();

            let result = read_object(&mut buf).await;

            let mut read_cur = Cursor::new(&buf_clone[..]);
            let object = subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            assert_eq!(result, SubgroupStreamObjectProcessResult::Success(object));
        }

        #[tokio::test]
        async fn stream_object_subgroup_continue_insufficient_payload() {
            let bytes_array = [
                0,  // Object ID (i)
                0,  // Extension Headers Length (i)
                50, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = read_object(&mut buf).await;

            assert_eq!(result, SubgroupStreamObjectProcessResult::Continue);
        }

        #[tokio::test]
        async fn stream_object_subgroup_continue_incomplete_message() {
            let bytes_array = [
                0, // Object ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);

            let result = read_object(&mut buf).await;

            assert_eq!(result, SubgroupStreamObjectProcessResult::Continue);
        }
    }
}

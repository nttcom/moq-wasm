use crate::{
    constants::TerminationErrorCode,
    modules::moqt_client::{MOQTClient, MOQTClientStatus},
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{
        stream_header_subgroup::StreamHeaderSubgroup, stream_header_track::StreamHeaderTrack,
        DataStreams,
    },
    variable_integer::read_variable_integer,
};
use std::{io::Cursor, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum StreamHeaderProcessResult {
    Success(StreamHeader),
    Continue,
    Failure(TerminationErrorCode, String),
}

#[derive(Debug, PartialEq)]
pub enum StreamHeader {
    Track(StreamHeaderTrack),
    Subgroup(StreamHeaderSubgroup),
}

fn read_data_stream_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<DataStreamType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let data_stream_type: DataStreamType = match DataStreamType::try_from(type_value) {
        Ok(v) => {
            if v == DataStreamType::ObjectDatagram {
                bail!("{:?} is not data stream type", v);
            }
            v
        }
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(data_stream_type)
}

pub async fn try_read_header(
    buf: &mut BytesMut,
    client: Arc<Mutex<MOQTClient>>,
) -> StreamHeaderProcessResult {
    let payload_length = buf.len();
    tracing::trace!("try to read stream header! {}", payload_length);

    // Check if the data stream type is exist
    if payload_length == 0 {
        return StreamHeaderProcessResult::Continue;
    }

    // check subscription and judge if it is invalid timing
    let client_status = client.lock().await.status();
    if client_status != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return StreamHeaderProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            message,
        );
    }

    let mut read_cur = Cursor::new(&buf[..]);

    // Read the data stream type
    let data_stream_type = match read_data_stream_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            buf.advance(read_cur.position() as usize);

            tracing::error!("data_stream_type is wrong: {:?}", err);
            return StreamHeaderProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received data stream type: {:?}", data_stream_type);

    match data_stream_type {
        DataStreamType::StreamHeaderTrack => {
            let result = StreamHeaderTrack::depacketize(&mut read_cur);
            match result {
                Ok(header) => {
                    buf.advance(read_cur.position() as usize);

                    let header = StreamHeader::Track(header);
                    StreamHeaderProcessResult::Success(header)
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    // Reset the cursor position because data for the header has not yet arrived
                    buf.advance(read_cur.position() as usize);
                    StreamHeaderProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    )
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            let result = StreamHeaderSubgroup::depacketize(&mut read_cur);
            match result {
                Ok(header) => {
                    buf.advance(read_cur.position() as usize);

                    let header = StreamHeader::Subgroup(header);
                    StreamHeaderProcessResult::Success(header)
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    // Reset the cursor position because data for the header has not yet arrived
                    buf.advance(read_cur.position() as usize);
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

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::{
            message_handlers::stream_header::{
                try_read_header, StreamHeader, StreamHeaderProcessResult,
            },
            moqt_client::{MOQTClient, MOQTClientStatus},
            server_processes::senders,
        };
        use bytes::BytesMut;
        use moqt_core::{
            data_stream_type::DataStreamType,
            messages::data_streams::{
                stream_header_subgroup::StreamHeaderSubgroup,
                stream_header_track::StreamHeaderTrack, DataStreams,
            },
            variable_integer::write_variable_integer,
        };
        use std::{io::Cursor, sync::Arc};
        use tokio::sync::Mutex;

        #[tokio::test]
        async fn stream_header_track_success() {
            let data_stream_type = DataStreamType::StreamHeaderTrack;
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Subscriber Priority (8)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_header(&mut buf, client).await;

            let mut buf_without_type = BytesMut::with_capacity(bytes_array.len());
            buf_without_type.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf_without_type[..]);
            let header = StreamHeaderTrack::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                StreamHeaderProcessResult::Success(StreamHeader::Track(header))
            );
        }

        #[tokio::test]
        async fn stream_header_subgroup_success() {
            let data_stream_type = DataStreamType::StreamHeaderSubgroup;
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Subgroup ID (i)
                4, // Subscriber Priority (8)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_header(&mut buf, client).await;

            let mut buf_without_type = BytesMut::with_capacity(bytes_array.len());
            buf_without_type.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf_without_type[..]);
            let header = StreamHeaderSubgroup::depacketize(&mut read_cur).unwrap();

            assert_eq!(
                result,
                StreamHeaderProcessResult::Success(StreamHeader::Subgroup(header))
            );
        }

        #[tokio::test]
        async fn stream_header_track_continue_incomplete_message() {
            let data_stream_type = DataStreamType::StreamHeaderTrack;
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_header(&mut buf, client).await;

            assert_eq!(result, StreamHeaderProcessResult::Continue);
        }

        #[tokio::test]
        async fn stream_header_subgroup_continue_incomplete_message() {
            let data_stream_type = DataStreamType::StreamHeaderSubgroup;
            let bytes_array = [
                0, // Object ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_header(&mut buf, client).await;

            assert_eq!(result, StreamHeaderProcessResult::Continue);
        }
    }
}

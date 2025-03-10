use crate::constants::TerminationErrorCode;
use crate::modules::moqt_client::MOQTClient;
use crate::modules::moqt_client::MOQTClientStatus;
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{datagram, DataStreams},
    variable_integer::read_variable_integer,
};
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum DatagramObjectProcessResult {
    Success(datagram::Object),
    Continue,
    Failure(TerminationErrorCode, String),
}

fn read_data_stream_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<DataStreamType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };
    let data_stream_type = DataStreamType::try_from(type_value)
        .map_err(|err| anyhow::anyhow!("Failed to convert value: {}", err))?;
    Ok(data_stream_type)
}

pub(crate) async fn try_read_object(
    buf: &mut BytesMut,
    client: Arc<Mutex<MOQTClient>>,
) -> DatagramObjectProcessResult {
    let payload_length = buf.len();
    tracing::trace!("try to read datagram object {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return DatagramObjectProcessResult::Continue;
    }

    // check subscription and judge if it is invalid timing
    let client_status = client.lock().await.status();
    if client_status != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return DatagramObjectProcessResult::Failure(
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

            return DatagramObjectProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };

    let result = match data_stream_type {
        DataStreamType::ObjectDatagram => datagram::Object::depacketize(&mut read_cur),
        _ => {
            return DatagramObjectProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Invalid message type: {:?}", data_stream_type),
            );
        }
    };
    match result {
        Ok(object) => {
            buf.advance(read_cur.position() as usize);
            DatagramObjectProcessResult::Success(object)
        }
        Err(err) => {
            tracing::warn!("{:#?}", err);
            // Reset the cursor position because data for an object has not yet arrived
            read_cur.set_position(0);
            DatagramObjectProcessResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::{
            message_handlers::datagram_object::{try_read_object, DatagramObjectProcessResult},
            moqt_client::{MOQTClient, MOQTClientStatus},
            server_processes::senders,
        };
        use bytes::BytesMut;
        use moqt_core::{
            data_stream_type::DataStreamType,
            messages::data_streams::{datagram, DataStreams},
            variable_integer::write_variable_integer,
        };
        use std::{io::Cursor, sync::Arc};
        use tokio::sync::Mutex;

        #[tokio::test]
        async fn datagram_object_success() {
            let data_stream_type = DataStreamType::ObjectDatagram;
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Extension Headers Length (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_object(&mut buf, client).await;

            println!("{:?}", result);

            let mut buf_without_type = BytesMut::with_capacity(bytes_array.len());
            buf_without_type.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf_without_type[..]);
            let datagram_object = datagram::Object::depacketize(&mut read_cur);

            println!("{:?}", datagram_object);

            assert_eq!(
                result,
                DatagramObjectProcessResult::Success(datagram_object.unwrap())
            );
        }

        #[tokio::test]
        async fn datagram_object_continue_insufficient_payload() {
            let data_stream_type = DataStreamType::ObjectDatagram;
            let bytes_array = [
                1,  // Track Alias (i)
                2,  // Group ID (i)
                3,  // Object ID (i)
                4,  // Subscriber Priority (8)
                0,  // Extension Headers Length (i)
                50, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_object(&mut buf, client).await;

            assert_eq!(result, DatagramObjectProcessResult::Continue);
        }

        #[tokio::test]
        async fn datagram_object_continue_incomplete_message() {
            let data_stream_type = DataStreamType::ObjectDatagram;
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
            buf.extend(write_variable_integer(data_stream_type as u64));
            buf.extend_from_slice(&bytes_array);

            let senders_mock = senders::test_helper_fn::create_senders_mock();
            let upstream_session_id = 0;

            let mut client = MOQTClient::new(upstream_session_id, senders_mock);
            client.update_status(MOQTClientStatus::SetUp);
            let client = Arc::new(Mutex::new(client));

            let result = try_read_object(&mut buf, client).await;

            assert_eq!(result, DatagramObjectProcessResult::Continue);
        }
    }
}

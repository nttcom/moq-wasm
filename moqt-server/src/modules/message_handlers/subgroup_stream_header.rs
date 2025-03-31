use crate::{
    constants::TerminationErrorCode,
    modules::moqt_client::{MOQTClient, MOQTClientStatus},
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{subgroup_stream, DataStreams},
    variable_integer::read_variable_integer,
};
use std::{io::Cursor, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum SubgroupStreamHeaderProcessResult {
    Success(subgroup_stream::Header),
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

pub async fn try_read_header(
    buf: &mut BytesMut,
    client: Arc<Mutex<MOQTClient>>,
) -> SubgroupStreamHeaderProcessResult {
    let payload_length = buf.len();
    tracing::trace!("try to read stream header! {}", payload_length);

    // Check if the data stream type is exist
    if payload_length == 0 {
        return SubgroupStreamHeaderProcessResult::Continue;
    }

    // check subscription and judge if it is invalid timing
    let client_status = client.lock().await.status();
    if client_status != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return SubgroupStreamHeaderProcessResult::Failure(
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
            return SubgroupStreamHeaderProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received data stream type: {:?}", data_stream_type);

    if data_stream_type != DataStreamType::SubgroupHeader {
        return SubgroupStreamHeaderProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            format!("Unknown message type: {:?}", data_stream_type),
        );
    }

    match subgroup_stream::Header::depacketize(&mut read_cur) {
        Ok(stream_header) => {
            buf.advance(read_cur.position() as usize);
            SubgroupStreamHeaderProcessResult::Success(stream_header)
        }
        Err(err) => {
            tracing::warn!("{:#?}", err);
            // Reset the cursor position because data for an object has not yet arrived
            read_cur.set_position(0);
            SubgroupStreamHeaderProcessResult::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::{
            message_handlers::subgroup_stream_header::{
                try_read_header, SubgroupStreamHeaderProcessResult,
            },
            moqt_client::{MOQTClient, MOQTClientStatus},
            server_processes::senders,
        };
        use bytes::BytesMut;
        use moqt_core::{
            data_stream_type::DataStreamType,
            messages::data_streams::{subgroup_stream, DataStreams},
            variable_integer::write_variable_integer,
        };
        use std::{io::Cursor, sync::Arc};
        use tokio::sync::Mutex;

        #[tokio::test]
        async fn subgroup_stream_header_success() {
            let data_stream_type = DataStreamType::SubgroupHeader;
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
            let header = subgroup_stream::Header::depacketize(&mut read_cur).unwrap();

            assert_eq!(result, SubgroupStreamHeaderProcessResult::Success(header));
        }

        #[tokio::test]
        async fn subgroup_stream_header_continue_incomplete_message() {
            let data_stream_type = DataStreamType::SubgroupHeader;
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

            assert_eq!(result, SubgroupStreamHeaderProcessResult::Continue);
        }
    }
}

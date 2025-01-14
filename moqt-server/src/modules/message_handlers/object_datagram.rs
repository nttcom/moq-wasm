use crate::constants::TerminationErrorCode;
use crate::modules::moqt_client::MOQTClient;
use crate::modules::moqt_client::MOQTClientStatus;
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    data_stream_type::DataStreamType,
    messages::data_streams::{object_datagram::ObjectDatagram, DataStreams},
    variable_integer::read_variable_integer,
};
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::Mutex;
#[derive(Debug, PartialEq)]
pub enum ObjectDatagramProcessResult {
    Success(ObjectDatagram),
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

    let data_stream_type: DataStreamType = match DataStreamType::try_from(type_value) {
        Ok(v) => {
            if v == DataStreamType::StreamHeaderTrack || v == DataStreamType::StreamHeaderSubgroup {
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

pub(crate) async fn try_read_object(
    buf: &mut BytesMut,
    client: Arc<Mutex<MOQTClient>>,
) -> ObjectDatagramProcessResult {
    let payload_length = buf.len();
    tracing::trace!("try to read datagram object {}", payload_length);

    // Check if the data is exist
    if payload_length == 0 {
        return ObjectDatagramProcessResult::Continue;
    }

    // check subscription and judge if it is invalid timing
    let client_status = client.lock().await.status();
    if client_status != MOQTClientStatus::SetUp {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return ObjectDatagramProcessResult::Failure(
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
            return ObjectDatagramProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };

    match data_stream_type {
        DataStreamType::ObjectDatagram => {
            let result = ObjectDatagram::depacketize(&mut read_cur);
            match result {
                Ok(object) => {
                    buf.advance(read_cur.position() as usize);

                    ObjectDatagramProcessResult::Success(object)
                }
                Err(err) => {
                    tracing::warn!("{:#?}", err);
                    // Reset the cursor position because data for an object has not yet arrived
                    read_cur.set_position(0);
                    ObjectDatagramProcessResult::Continue
                }
            }
        }
        _ => ObjectDatagramProcessResult::Failure(
            TerminationErrorCode::ProtocolViolation,
            format!("Invalid message type: {:?}", data_stream_type),
        ),
    }
}

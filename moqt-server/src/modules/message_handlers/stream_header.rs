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
    IncompleteMessage,
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
        return StreamHeaderProcessResult::IncompleteMessage;
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

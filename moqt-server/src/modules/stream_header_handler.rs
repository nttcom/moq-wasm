use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::modules::server_processes::stream_track_header::process_stream_header_track;
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::UnderlayType, data_stream_type::DataStreamType,
    variable_integer::read_variable_integer, MOQTClient, SendStreamDispatcherRepository,
};

#[derive(Debug, PartialEq)]
pub enum MessageProcessResult {
    Success(BytesMut),
    SuccessWithoutResponse,
    Failure(TerminationErrorCode, String),
    Fragment,
}

fn read_header_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<DataStreamType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let header_type: DataStreamType = match DataStreamType::try_from(type_value) {
        Ok(v) => {
            if v == DataStreamType::ObjectDatagram {
                bail!("{:?} is not header type", v);
            }
            v
        }
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(header_type)
}

pub async fn stream_header_handler(
    read_buf: &mut BytesMut,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> MessageProcessResult {
    tracing::trace!("stream_header_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::debug!("read_cur! {:?}", read_cur);

    // Read the header type
    let header_type = match read_header_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("header_type is wrong {:?}", err);
            return MessageProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received Header Type: {:?}", header_type);

    // check subscription and judge if it is invalid timing
    // set uni stream sender

    let mut write_buf = BytesMut::new();

    match header_type {
        DataStreamType::StreamHeaderTrack => {
            match process_stream_header_track(
                &mut read_buf,
                pubsub_relation_manager_repository,
                send_stream_dispatcher_repository,
                &mut client,
            )
            .await
            {
                Ok(_) => ControlMessageType::ServerSetup,
                Err(err) => {
                    // TODO: To ensure future extensibility of MOQT, the peers MUST ignore unknown setup parameters.
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        DataStreamType::StreamHeaderSubgroup => {
            unimplemented!();
        }
        unknown => {
            return MessageProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    MessageProcessResult::Success(message_buf)
}

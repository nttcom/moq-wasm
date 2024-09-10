use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::handlers::announce_handler::AnnounceResponse;
use crate::modules::handlers::unannounce_handler::unannounce_handler;
use crate::modules::messages::unannounce::UnAnnounce;
use crate::server_processes::announce_message::process_announce_message;
use crate::server_processes::client_setup_message::process_client_setup_message;
use crate::server_processes::object_message::{
    process_object_with_payload_length, process_object_without_payload_length,
};
use crate::server_processes::subscribe_message::process_subscribe_message;
use crate::server_processes::subscribe_ok_message::process_subscribe_ok_message;

use super::constants::UnderlayType;
use super::message_type::MessageType;
use super::messages::moqt_payload::MOQTPayload;
use super::moqt_client::{MOQTClient, MOQTClientStatus};
use super::send_stream_dispatcher_repository::SendStreamDispatcherRepository;
use super::track_namespace_manager_repository::TrackNamespaceManagerRepository;
use super::variable_integer::{read_variable_integer, write_variable_integer};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Uni,
    Bi,
}

#[derive(Debug)]
pub enum MessageProcessResult {
    Success(BytesMut),
    SuccessWithoutResponse,
    Failure(TerminationErrorCode, String),
    Fragment,
}

fn read_message_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<MessageType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let message_type: MessageType = match MessageType::try_from(type_value) {
        Ok(v) => v,
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(message_type)
}

pub async fn message_handler(
    read_buf: &mut BytesMut,
    stream_type: StreamType,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> MessageProcessResult {
    tracing::trace!("message_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::debug!("read_cur! {:?}", read_cur);

    // Read the message type
    let message_type = match read_message_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("message_type is wrong {:?}", err);
            return MessageProcessResult::Failure(
                TerminationErrorCode::GenericError,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received Message Type: {:?}", message_type);
    if message_type.is_setup_message() {
        // Setup message must be sent on bidirectional stream
        if stream_type == StreamType::Uni {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Setup message must be sent on bidirectional stream");
            tracing::debug!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
        }
    } else if message_type.is_control_message() {
        // TODO: Drop with Protocol Violation if it is a different stream from the SETUP message
    } else {
        // Object message must be sent on unidirectional stream
        if stream_type == StreamType::Bi {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Object message must be sent on unidirectional stream");
            tracing::debug!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, message);
        }
    }

    if read_cur.remaining() == 0 {
        // The length is insufficient, so do nothing. Do not synchronize with the cursor.
        tracing::error!("fragmented {}", read_buf.len());
        return MessageProcessResult::Fragment;
    }

    let payload_length = read_cur.remaining();
    read_buf.advance(read_cur.position() as usize);
    let mut payload_buf = read_buf.split_to(payload_length);
    let mut write_buf = BytesMut::new();

    let return_message_type = match message_type {
        MessageType::ObjectWithPayloadLength => {
            match process_object_with_payload_length(
                &mut payload_buf,
                client,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::ObjectWithoutPayloadLength => {
            match process_object_without_payload_length(
                &mut payload_buf,
                client,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::ClientSetup => {
            match process_client_setup_message(
                &mut payload_buf,
                client,
                underlay_type,
                &mut write_buf,
            ) {
                Ok(_) => MessageType::ServerSetup,
                Err(err) => {
                    // TODO: To ensure future extensibility of MOQT, the peers MUST ignore unknown setup parameters.
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::Subscribe => {
            match process_subscribe_message(
                &mut payload_buf,
                client,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::SubscribeOk => {
            match process_subscribe_ok_message(
                &mut payload_buf,
                client,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // MessageType::SubscribeError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnSubscribe => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            let unsubscribe_message = UnAnnounce::depacketize(&mut payload_buf);

            if let Err(err) = unsubscribe_message {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: Not implemented yet
            let _unsubscribe_result = unannounce_handler(
                unsubscribe_message.unwrap(),
                client,
                track_namespace_manager_repository,
            );

            return MessageProcessResult::Success(BytesMut::with_capacity(0));
        }
        MessageType::Announce => {
            match process_announce_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                track_namespace_manager_repository,
            )
            .await
            {
                Ok(announce_result) => match announce_result {
                    AnnounceResponse::Success(_) => MessageType::AnnounceOk,
                    AnnounceResponse::Failure(_) => MessageType::AnnounceError,
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // MessageType::AnnounceOk => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        // MessageType::AnnounceError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnAnnounce => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            let unannounce_message = UnAnnounce::depacketize(&mut payload_buf);

            if let Err(err) = unannounce_message {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: Not implemented yet
            let _unannounce_result = unannounce_handler(
                unannounce_message.unwrap(),
                client,
                track_namespace_manager_repository,
            )
            .await;

            return MessageProcessResult::Success(BytesMut::with_capacity(0));
        }
        MessageType::GoAway => {
            todo!("GoAway");
        }
        unknown => {
            return MessageProcessResult::Failure(
                TerminationErrorCode::GenericError,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    tracing::info!("Return Message Type: {:?}", return_message_type.clone());
    let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);
    // Add type
    message_buf.extend(write_variable_integer(u8::from(return_message_type) as u64));
    // Add payload
    message_buf.extend(write_buf);

    tracing::debug!("message_buf: {:#x?}", message_buf);

    MessageProcessResult::Success(message_buf)
}

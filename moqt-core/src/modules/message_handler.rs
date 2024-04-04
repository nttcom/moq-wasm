use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::messages::object_message::{
    ObjectMessageWithPayloadLength, ObjectMessageWithoutPayloadLength,
};
use crate::modules::handlers::subscribe_handler::subscribe_handler;
use crate::modules::handlers::unannounce_handler::unannounce_handler;
use crate::modules::messages::announce_message::AnnounceMessage;
use crate::modules::messages::client_setup_message::ClientSetupMessage;
use crate::modules::messages::subscribe_request_message::SubscribeRequestMessage;
use crate::modules::messages::unannounce_message::UnAnnounceMessage;

use super::constants::UnderlayType;
use super::handlers::announce_handler::announce_handler;
use super::handlers::server_setup_handler::setup_handler;
use super::message_type::MessageType;
use super::messages::moqt_payload::MOQTPayload;
use super::moqt_client::{MOQTClient, MOQTClientStatus};
use super::track_manager_repository::TrackManagerRepository;
use super::variable_integer::{read_variable_integer, write_variable_integer};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Uni,
    Bi,
}

pub enum MessageProcessResult {
    Success(BytesMut),
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

#[tracing::instrument(name="StableID",skip_all,fields(id=client.id()))]
pub async fn message_handler(
    read_buf: &mut BytesMut,
    stream_type: StreamType,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> MessageProcessResult {
    tracing::info!("message_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::info!("read_cur! {:?}", read_cur);

    let message_type = match read_message_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::info!("message_type is wrong {:?}", err);
            return MessageProcessResult::Failure(
                TerminationErrorCode::GenericError,
                err.to_string(),
            );
        }
    };

    tracing::info!("Message Type: {:?}", message_type);

    if message_type.is_setup_message() {
        // Setup message must be sent on bidirectional stream
        if stream_type == StreamType::Uni {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Setup message must be sent on bidirectional stream");
            tracing::info!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
        }
    } else if message_type.is_control_message() {
        // TODO: SETUPメッセージと異なるストリームだったらProtocol Violationで落とす
    } else {
        // Object message must be sent on unidirectional stream
        if stream_type == StreamType::Bi {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Object message must be sent on unidirectional stream");
            tracing::info!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, message);
        }
    }

    if read_cur.remaining() == 0 {
        // 長さが足りないので何もしない。cursorと同期もしない
        tracing::info!("fragmented {}", read_buf.len());
        return MessageProcessResult::Fragment;
    }

    let payload_length = read_cur.remaining();
    read_buf.advance(read_cur.position() as usize);

    // payload相当の部分だけ切り出す
    let mut payload_buf = read_buf.split_to(payload_length);
    let mut write_buf = BytesMut::new();

    // 各メッセージでクラス化
    // 自分はサーバであるととりあえず仮定
    let return_message_type = match message_type {
        MessageType::ObjectWithLength => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            // FIXME: 仮でechoする
            match ObjectMessageWithPayloadLength::depacketize(&mut payload_buf) {
                Ok(object_message) => {
                    object_message.packetize(&mut write_buf);
                    MessageType::ObjectWithLength
                }
                Err(err) => {
                    tracing::info!("{:#?}", err);
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::ObjectWithoutLength => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            // FIXME: 仮でechoする
            match ObjectMessageWithoutPayloadLength::depacketize(&mut payload_buf) {
                Ok(object_message) => {
                    object_message.packetize(&mut write_buf);

                    MessageType::ObjectWithoutLength
                }
                Err(err) => {
                    // fix
                    tracing::info!("{:#?}", err);
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::ClientSetup => {
            if client.status() != MOQTClientStatus::Connected {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }
            let client_setup_message = match ClientSetupMessage::depacketize(&mut payload_buf) {
                Ok(client_setup_message) => client_setup_message,
                Err(err) => {
                    tracing::info!("{:#?}", err);
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            };

            match setup_handler(client_setup_message, underlay_type, client) {
                Ok(server_setup_message) => {
                    server_setup_message.packetize(&mut write_buf);
                    MessageType::ServerSetup
                }
                Err(err) => {
                    tracing::info!("{:#?}", err);
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::Subscribe => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            let subscribe_request_message = SubscribeRequestMessage::depacketize(&mut payload_buf);

            if let Err(err) = subscribe_request_message {
                // fix
                tracing::info!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            let subscribe_result = subscribe_handler(
                subscribe_request_message.unwrap(),
                client,
                track_manager_repository,
            )
            .await;

            match subscribe_result {
                Ok(subscribe_response) => match subscribe_response {
                    crate::modules::handlers::subscribe_handler::SubscribeResponse::Success(
                        subscribe_ok,
                    ) => {
                        subscribe_ok.packetize(&mut write_buf);
                        MessageType::SubscribeOk
                    }
                    crate::modules::handlers::subscribe_handler::SubscribeResponse::Failure(
                        subscribe_error,
                    ) => {
                        subscribe_error.packetize(&mut write_buf);
                        MessageType::SubscribeError
                    }
                },
                Err(err) => {
                    // fix
                    tracing::info!("{:#?}", err);
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // MessageType::SubscribeOk => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
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

            let unsubscribe_message = UnAnnounceMessage::depacketize(&mut payload_buf);

            if let Err(err) = unsubscribe_message {
                // fix
                tracing::info!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: 未実装のため_をつけている
            let _unsubscribe_result = unannounce_handler(
                unsubscribe_message.unwrap(),
                client,
                track_manager_repository,
            );

            return MessageProcessResult::Success(BytesMut::with_capacity(0));
        }
        MessageType::Announce => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            let announce_message = AnnounceMessage::depacketize(&mut payload_buf);

            if let Err(err) = announce_message {
                // fix
                tracing::info!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            let announce_result =
                announce_handler(announce_message.unwrap(), client, track_manager_repository).await;

            match announce_result {
                Ok(announce_message) => match announce_message {
                    crate::modules::handlers::announce_handler::AnnounceResponse::Success(
                        ok_message,
                    ) => {
                        ok_message.packetize(&mut write_buf);
                        MessageType::AnnounceOk
                    }
                    crate::modules::handlers::announce_handler::AnnounceResponse::Failure(
                        err_message,
                    ) => {
                        err_message.packetize(&mut write_buf);
                        MessageType::AnnounceError
                    }
                },
                Err(err) => {
                    // fix
                    tracing::info!("{:#?}", err);
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

            let unannounce_message = UnAnnounceMessage::depacketize(&mut payload_buf);

            if let Err(err) = unannounce_message {
                // fix
                tracing::info!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: 未実装のため_をつけている
            let _unannounce_result = unannounce_handler(
                unannounce_message.unwrap(),
                client,
                track_manager_repository,
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

    let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);
    // Add type
    message_buf.extend(write_variable_integer(u8::from(return_message_type) as u64));
    // Add payload
    message_buf.extend(write_buf);

    tracing::info!("message_buf: {:#x?}", message_buf);

    MessageProcessResult::Success(message_buf)
}

use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::handlers::announce_handler::AnnounceResponse;
use crate::messages::object_message::{
    ObjectMessageWithPayloadLength, ObjectMessageWithoutPayloadLength,
};
use crate::modules::handlers::unannounce_handler::unannounce_handler;
use crate::modules::messages::unannounce_message::UnAnnounceMessage;
use crate::server_processes::announce_message::process_announce_message;
use crate::server_processes::client_setup_message::process_client_setup_message;
use crate::server_processes::subscribe_message::process_subscribe_message;

use super::constants::UnderlayType;
use super::message_type::MessageType;
use super::messages::moqt_payload::MOQTPayload;
use super::moqt_client::{MOQTClient, MOQTClientStatus};
use super::stream_manager_repository::StreamManagerRepository;
use super::track_namespace_manager_repository::TrackNamespaceManagerRepository;
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

#[tracing::instrument(name="StableID",skip_all,fields(id=client.id()))]
pub async fn message_handler(
    read_buf: &mut BytesMut,
    stream_type: StreamType,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    stream_manager_repository: &mut dyn StreamManagerRepository,
) -> MessageProcessResult {
    tracing::info!("message_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::info!("read_cur! {:?}", read_cur);

    // メッセージタイプを読む
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

    // ペイロードを読む
    let payload_length = read_cur.remaining();
    read_buf.advance(read_cur.position() as usize);

    // payload相当の部分だけ切り出す
    let mut payload_buf = read_buf.split_to(payload_length);
    let mut write_buf = BytesMut::new();

    // 各メッセージでクラス化
    // 自分はサーバであるととりあえず仮定
    let return_message_type = match message_type {
        MessageType::ObjectWithLength => {
            // TODO: server_processesフォルダに移管する
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
            // TODO: server_processesフォルダに移管する
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
                track_manager_repository,
                stream_manager_repository,
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
        // MessageType::SubscribeOk => {
        //       // TODO: server_processesフォルダに移管する
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        // MessageType::SubscribeError => {
        //      // TODO: server_processesフォルダに移管する
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnSubscribe => {
            // TODO: server_processesフォルダに移管する
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
            match process_announce_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                track_manager_repository,
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
        //     // TODO: server_processesフォルダに移管する
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        // MessageType::AnnounceError => {
        //     // TODO: server_processesフォルダに移管する
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnAnnounce => {
            // TODO: server_processesフォルダに移管する
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
            // TODO: server_processesフォルダに移管する
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

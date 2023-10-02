use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::modules::messages::setup_message::ClientSetupMessage;

use super::constants::UnderlayType;
use super::handlers::server_setup_handler::setup_handler;
use super::message_type::MessageType;
use super::messages::payload::Payload;
use super::moqt_client::{MOQTClient, MOQTClientStatus};
use super::variable_integer::{read_variable_integer, write_variable_integer};
use bytes::{Buf, BytesMut};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamType {
    Uni,
    Bi,
}

pub(crate) enum MessageProcessResult {
    Success(BytesMut),
    Failure(TerminationErrorCode, String),
    Fragment,
}

#[tracing::instrument(name="StableID",skip_all,fields(id=client.id()))]
pub(crate) fn message_handler(
    read_buf: &mut BytesMut,
    stream_type: StreamType,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
) -> MessageProcessResult {
    // tracing::info!("message_handler!");
    tracing::info!("message_handler! {}", read_buf.len());

    // ちゃんと読んだ場合はread_bufも対応してupdateが必要
    let mut read_cur = Cursor::new(&read_buf[..]);

    // typeを読む
    let type_value = read_variable_integer(&mut read_cur);
    if let Err(err) = type_value {
        read_buf.advance(read_cur.position() as usize);

        tracing::info!("{:?}", err);
        return MessageProcessResult::Failure(TerminationErrorCode::GenericError, err.to_string());
    }
    let type_value = type_value.unwrap();

    let type_value = u8::try_from(type_value);
    if let Err(err) = type_value {
        read_buf.advance(read_cur.position() as usize);

        tracing::info!("message_type is not u8 {:?}", err);
        return MessageProcessResult::Failure(TerminationErrorCode::GenericError, err.to_string());
    }
    let type_value = type_value.unwrap();

    let message_type: MessageType = match MessageType::try_from(type_value) {
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

    // Setup message must be sent on bidirectional stream
    if message_type == MessageType::Setup && stream_type == StreamType::Uni {
        read_buf.advance(read_cur.position() as usize);

        let message = String::from("Setup message must be sent on bidirectional stream");
        tracing::info!(message);
        return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
    }

    if read_cur.remaining() == 0 {
        // 長さが足りないので何もしない。cursorと同期もしない
        tracing::info!("fragmented {}", read_buf.len());
        return MessageProcessResult::Fragment;
    }

    // lenを読む
    let payload_length = read_variable_integer(&mut read_cur);
    if let Err(err) = payload_length {
        read_buf.advance(read_cur.position() as usize);

        tracing::info!("{:?}", err);
        return MessageProcessResult::Failure(TerminationErrorCode::GenericError, err.to_string());
    }
    let payload_length = payload_length.unwrap() as usize;

    let rest_buf_len = read_buf.len() - (read_cur.position() as usize);
    if rest_buf_len < payload_length {
        // 長さが足りないので何もしない。cursorと同期もしない
        tracing::info!("fragmented {} {}", rest_buf_len, payload_length);
        return MessageProcessResult::Fragment;
    }

    read_buf.advance(read_cur.position() as usize);

    // payload相当の部分だけ切り出す
    // payload長が間違っていると後ろが狂うが、困るのはこのclientだけなので許容
    let mut payload_buf = read_buf.split_to(payload_length);
    let mut write_buf = BytesMut::new();

    // 各メッセージでクラス化
    // 自分はサーバであるととりあえず仮定
    match message_type {
        // MessageType::Object => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::Setup => {
            if client.status() != MOQTClientStatus::Connected {
                let message = String::from("Invalid timing");
                tracing::info!(message);
                return MessageProcessResult::Failure(TerminationErrorCode::GenericError, message);
            }

            let client_setup_message = ClientSetupMessage::depacketize(&mut payload_buf);
            if let Err(err) = client_setup_message {
                tracing::info!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }
            let client_setup_message = client_setup_message.unwrap();

            let setup_result = setup_handler(client_setup_message, underlay_type, client);
            match setup_result {
                Ok(server_setup_message) => {
                    server_setup_message.packetize(&mut write_buf);
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
        // MessageType::SubscribeRequest => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
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
        // MessageType::Announce => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
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
        // MessageType::GoAway => {}
        _ => {}
    };

    let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);
    // Add type
    message_buf.extend(write_variable_integer(u8::from(message_type) as u64));
    // Add length
    message_buf.extend(write_variable_integer(write_buf.len() as u64));
    // Add payload
    message_buf.extend(write_buf);

    MessageProcessResult::Success(message_buf)
}

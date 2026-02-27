use std::io::Cursor;

use bytes::{Buf, BytesMut};
use tokio_util::codec::Decoder;

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, result_ext::ResultExt},
    moqt::{
        control_plane::control_messages::{
            control_message_type::ControlMessageType,
            messages::{
                client_setup::ClientSetup, namespace_ok::NamespaceOk, publish::Publish,
                publish_namespace::PublishNamespace, publish_ok::PublishOk,
                request_error::RequestError, server_setup::ServerSetup, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
            },
        },
        data_plane::streams::stream::received_message::ReceivedMessage,
    },
};

pub(crate) struct MessageDecoder;

impl Decoder for MessageDecoder {
    type Item = ReceivedMessage;
    // TODO: define a proper error type.
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut cursor_buf = Cursor::new(&src[..]);
        let message_type = self.get_message_type(&mut cursor_buf).map_err(|e| {
            tracing::error!("Failed to get message type: {:?}", e);
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Failed to get message type",
            )
        })?;
        if message_type.is_none() {
            return Ok(None);
        }
        let message_type = message_type.unwrap();
        if let Err(e) = self.validate_message_length(&mut cursor_buf) {
            tracing::error!("Failed to validate message length: {:?}", e);
            return Ok(None);
        }
        let result = self.resolve_message(message_type, &mut cursor_buf);
        src.split_to(cursor_buf.position() as usize);
        Ok(Some(result))
    }
}

impl MessageDecoder {
    fn get_message_type(
        &self,
        src: &mut Cursor<&[u8]>,
    ) -> anyhow::Result<Option<ControlMessageType>> {
        let message_type = if let Ok(message_type) = src.try_get_varint() {
            if let Ok(message_type) = ControlMessageType::try_from(message_type as u8) {
                message_type
            } else {
                anyhow::bail!("Invalid message type: {}", message_type);
            }
        } else {
            return Ok(None);
        };
        Ok(Some(message_type))
    }

    fn validate_message_length(&self, src: &mut Cursor<&[u8]>) -> anyhow::Result<()> {
        let payload_length = match src.try_get_u16().log_context("payload length") {
            Ok(v) => v,
            Err(_) => {
                tracing::error!("Failed to read payload length");
                return Err(anyhow::anyhow!("Failed to read payload length"));
            }
        };

        if src.remaining() < payload_length as usize {
            tracing::error!(
                "Message length unmatches. expect {}, actual {}",
                payload_length,
                src.remaining()
            );
            return Err(anyhow::anyhow!("Message length unmatches"));
        }
        Ok(())
    }

    fn resolve_message(
        &self,
        message_type: ControlMessageType,
        mut cursor_buf: &mut Cursor<&[u8]>,
    ) -> ReceivedMessage {
        tracing::debug!("Event: message_type: {:?}", message_type);
        match message_type {
            ControlMessageType::ClientSetup => {
                tracing::debug!("Event: Client setup");
                match ClientSetup::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::ClientSetup(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::ServerSetup => {
                tracing::debug!("Event: Server setup");
                match ServerSetup::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::ServerSetup(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::GoAway => todo!(),
            ControlMessageType::MaxSubscribeId => todo!(),
            ControlMessageType::RequestsBlocked => todo!(),
            ControlMessageType::Subscribe => {
                tracing::debug!("Event: Subscribe");
                match Subscribe::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::Subscribe(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::SubscribeOk => {
                tracing::debug!("Event: Subscribe ok");
                match SubscribeOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::SubscribeError => {
                tracing::debug!("Event: Subscribe error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::UnSubscribe => todo!(),
            ControlMessageType::PublishDone => todo!(),
            ControlMessageType::Publish => {
                tracing::debug!("Event: Publish");
                match Publish::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::Publish(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::PublishOk => {
                tracing::debug!("Event: Publish ok");
                match PublishOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::PublishError => {
                tracing::debug!("Event: Publish error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::Fetch => todo!(),
            ControlMessageType::FetchOk => todo!(),
            ControlMessageType::FetchError => todo!(),
            ControlMessageType::FetchCancel => todo!(),
            ControlMessageType::TrackStatusRequest => todo!(),
            ControlMessageType::TrackStatus => todo!(),
            ControlMessageType::PublishNamespace => {
                tracing::debug!("Event: Publish namespace");
                match PublishNamespace::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespace(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::PublishNamespaceOk => {
                tracing::debug!("Event: Publish namespace ok");
                match NamespaceOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespaceOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::PublishNamespaceError => {
                tracing::debug!("Event: Publish namespace error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespaceError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::PublishNamespaceDone => todo!(),
            ControlMessageType::PublishNamespaceCancel => todo!(),
            ControlMessageType::SubscribeNamespace => {
                tracing::debug!("Event: Subscribe namespace");
                match SubscribeNamespace::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeNamespace(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::SubscribeNamespaceOk => {
                tracing::debug!("Event: Subscribe namespace ok");
                match NamespaceOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeNamespaceOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::SubscribeNamespaceError => {
                tracing::debug!("Event: Subscribe namespace error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeNamespaceError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        return ReceivedMessage::FatalError();
                    }
                }
            }
            ControlMessageType::UnSubscribeNamespace => todo!(),
        }
    }
}

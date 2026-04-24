use std::io::Cursor;

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::moqt::{
    control_plane::control_messages::{
        control_message_type::ControlMessageType,
        messages::{
            client_setup::ClientSetup, namespace_ok::NamespaceOk, publish::Publish,
            publish_namespace::PublishNamespace, publish_ok::PublishOk,
            request_error::RequestError, server_setup::ServerSetup, subscribe::Subscribe,
            subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
            unsubscribe::Unsubscribe,
        },
    },
    data_plane::streams::stream::received_message::ReceivedMessage,
};
use crate::wire::take_control_message;

pub(crate) struct ControlMessageDecoder;

impl Decoder for ControlMessageDecoder {
    type Item = ReceivedMessage;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let (message_type, payload) = match take_control_message(src).map_err(|error| {
            tracing::error!("Failed to decode control message frame: {:?}", error);
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Failed to decode control message frame",
            )
        })? {
            Some(frame) => frame,
            None => return Ok(None),
        };

        Ok(Some(self.resolve_message(message_type, payload)))
    }
}

impl ControlMessageDecoder {
    fn resolve_message(
        &self,
        message_type: ControlMessageType,
        payload: BytesMut,
    ) -> ReceivedMessage {
        tracing::debug!("Event: message_type: {:?}", message_type);
        let mut cursor_buf = Cursor::new(payload.as_ref());

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
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::SubscribeError => {
                tracing::debug!("Event: Subscribe error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::SubscribeUpdate => todo!(),
            ControlMessageType::UnSubscribe => {
                tracing::debug!("Event: Unsubscribe");
                match Unsubscribe::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::Unsubscribe(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::PublishDone => todo!(),
            ControlMessageType::Publish => {
                tracing::debug!("Event: Publish");
                match Publish::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::Publish(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::PublishOk => {
                tracing::debug!("Event: Publish ok");
                match PublishOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::PublishError => {
                tracing::debug!("Event: Publish error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
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
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::PublishNamespaceOk => {
                tracing::debug!("Event: Publish namespace ok");
                match NamespaceOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespaceOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::PublishNamespaceError => {
                tracing::debug!("Event: Publish namespace error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespaceError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
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
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::SubscribeNamespaceOk => {
                tracing::debug!("Event: Subscribe namespace ok");
                match NamespaceOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeNamespaceOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::SubscribeNamespaceError => {
                tracing::debug!("Event: Subscribe namespace error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::SubscribeNamespaceError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::UnSubscribeNamespace => todo!(),
        }
    }
}

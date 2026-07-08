use std::io::Cursor;

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::moqt::{
    control_plane::control_messages::{
        control_message_type::ControlMessageType,
        messages::{
            client_setup::ClientSetup, fetch::Fetch, fetch_ok::FetchOk, namespace_ok::NamespaceOk,
            publish::Publish, publish_namespace::PublishNamespace,
            publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
            request_error::RequestError, server_setup::ServerSetup, subscribe::Subscribe,
            subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
            unsubscribe::Unsubscribe, unsubscribe_namespace::UnsubscribeNamespace,
        },
    },
    data_plane::stream::received_message::ReceivedMessage,
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
            ControlMessageType::Fetch => {
                tracing::debug!("Event: Fetch");
                match Fetch::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::Fetch(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::FetchOk => {
                tracing::debug!("Event: Fetch ok");
                match FetchOk::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::FetchOk(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
            ControlMessageType::FetchError => {
                tracing::debug!("Event: Fetch error");
                match RequestError::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::FetchError(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
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
            ControlMessageType::PublishNamespaceDone => {
                tracing::debug!("Event: Publish namespace done");
                match PublishNamespaceDone::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::PublishNamespaceDone(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
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
            ControlMessageType::UnSubscribeNamespace => {
                tracing::debug!("Event: Unsubscribe namespace");
                match UnsubscribeNamespace::decode(&mut cursor_buf) {
                    Some(v) => ReceivedMessage::UnsubscribeNamespace(v),
                    None => {
                        tracing::error!("Protocol violation is detected.");
                        ReceivedMessage::FatalError()
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::{BufMut, BytesMut};
    use tokio_util::codec::Decoder;

    use crate::modules::moqt::control_plane::control_messages::messages::parameters::{
        filter_type::FilterType, group_order::GroupOrder,
    };
    use crate::modules::moqt::control_plane::control_messages::messages::{
        subscribe::Subscribe, unsubscribe::Unsubscribe,
    };
    use crate::modules::moqt::data_plane::stream::received_message::ReceivedMessage;
    use crate::wire::{ControlMessageType, encode_control_message};

    use super::ControlMessageDecoder;

    fn make_subscribe() -> Subscribe {
        Subscribe {
            request_id: 7,
            track_namespace: vec!["test".to_string()],
            track_name: "track".to_string(),
            subscriber_priority: 0,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
            authorization_tokens: vec![],
            delivery_timeout: None,
        }
    }

    #[test]
    fn decode_returns_none_for_empty_buffer() {
        let mut decoder = ControlMessageDecoder;
        let mut buf = BytesMut::new();

        let result = decoder.decode(&mut buf).expect("decode should not fail");
        assert!(result.is_none());
    }

    #[test]
    fn decode_waits_for_full_frame_then_completes() {
        let mut decoder = ControlMessageDecoder;
        let subscribe = make_subscribe();
        let framed = encode_control_message(ControlMessageType::Subscribe, subscribe.encode());

        let mut buf = BytesMut::from(&framed[..framed.len() - 1]);
        let result = decoder.decode(&mut buf).expect("decode should not fail");
        assert!(result.is_none());
        // partial frame must stay in the buffer until the rest arrives
        assert_eq!(buf.len(), framed.len() - 1);

        buf.put_slice(&framed[framed.len() - 1..]);
        let message = decoder
            .decode(&mut buf)
            .expect("decode should not fail")
            .expect("frame should be complete");
        match message {
            ReceivedMessage::Subscribe(decoded) => assert_eq!(decoded, subscribe),
            other => panic!("Expected Subscribe, got {:?}", other),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_multiple_messages_in_one_buffer() {
        let mut decoder = ControlMessageDecoder;
        let subscribe = make_subscribe();
        let unsubscribe = Unsubscribe { request_id: 7 };

        let mut buf = BytesMut::new();
        buf.unsplit(encode_control_message(
            ControlMessageType::Subscribe,
            subscribe.encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::UnSubscribe,
            unsubscribe.encode(),
        ));

        let first = decoder
            .decode(&mut buf)
            .expect("decode should not fail")
            .expect("first frame should be complete");
        match first {
            ReceivedMessage::Subscribe(decoded) => assert_eq!(decoded, subscribe),
            other => panic!("Expected Subscribe, got {:?}", other),
        }

        let second = decoder
            .decode(&mut buf)
            .expect("decode should not fail")
            .expect("second frame should be complete");
        match second {
            ReceivedMessage::Unsubscribe(decoded) => assert_eq!(decoded, unsubscribe),
            other => panic!("Expected Unsubscribe, got {:?}", other),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_rejects_unknown_message_type() {
        let mut decoder = ControlMessageDecoder;
        // 0x3f is not a defined control message type
        let mut buf = BytesMut::new();
        buf.put_u8(0x3f);
        buf.put_u16(0);

        let result = decoder.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn decode_malformed_payload_yields_fatal_error() {
        let mut decoder = ControlMessageDecoder;
        // Subscribe frame whose payload is truncated garbage
        let payload = BytesMut::from(&[0x07_u8][..]);
        let mut buf = encode_control_message(ControlMessageType::Subscribe, payload);

        let message = decoder
            .decode(&mut buf)
            .expect("framing should succeed")
            .expect("frame should be complete");
        assert!(matches!(message, ReceivedMessage::FatalError()));
    }
}

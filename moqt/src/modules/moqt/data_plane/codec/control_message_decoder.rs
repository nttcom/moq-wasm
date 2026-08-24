use std::io::Cursor;

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::moqt::{
    control_plane::control_messages::{
        control_message_type::ControlMessageType,
        messages::{
            client_setup::ClientSetup, fetch::Fetch, fetch_cancel::FetchCancel, fetch_ok::FetchOk,
            go_away::GoAway, max_request_id::MaxRequestId, namespace_ok::NamespaceOk,
            publish::Publish, publish_done::PublishDone, publish_namespace::PublishNamespace,
            publish_namespace_cancel::PublishNamespaceCancel,
            publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
            request_error::RequestError, requests_blocked::RequestsBlocked,
            server_setup::ServerSetup, subscribe::Subscribe,
            subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
            subscribe_update::SubscribeUpdate, unsubscribe::Unsubscribe,
            unsubscribe_namespace::UnsubscribeNamespace,
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
        let mut cursor = Cursor::new(payload.as_ref());

        match message_type {
            ControlMessageType::ClientSetup => Self::decode_payload(
                &mut cursor,
                ClientSetup::decode,
                ReceivedMessage::ClientSetup,
            ),
            ControlMessageType::ServerSetup => Self::decode_payload(
                &mut cursor,
                ServerSetup::decode,
                ReceivedMessage::ServerSetup,
            ),
            ControlMessageType::GoAway => {
                Self::decode_payload(&mut cursor, GoAway::decode, ReceivedMessage::GoAway)
            }
            ControlMessageType::MaxRequestId => Self::decode_payload(
                &mut cursor,
                MaxRequestId::decode,
                ReceivedMessage::MaxRequestId,
            ),
            ControlMessageType::RequestsBlocked => Self::decode_payload(
                &mut cursor,
                RequestsBlocked::decode,
                ReceivedMessage::RequestsBlocked,
            ),
            ControlMessageType::Subscribe => {
                Self::decode_payload(&mut cursor, Subscribe::decode, ReceivedMessage::Subscribe)
            }
            ControlMessageType::SubscribeOk => Self::decode_payload(
                &mut cursor,
                SubscribeOk::decode,
                ReceivedMessage::SubscribeOk,
            ),
            ControlMessageType::SubscribeError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::SubscribeError,
            ),
            ControlMessageType::SubscribeUpdate => Self::decode_payload(
                &mut cursor,
                SubscribeUpdate::decode,
                ReceivedMessage::SubscribeUpdate,
            ),
            ControlMessageType::UnSubscribe => Self::decode_payload(
                &mut cursor,
                Unsubscribe::decode,
                ReceivedMessage::Unsubscribe,
            ),
            ControlMessageType::PublishDone => Self::decode_payload(
                &mut cursor,
                PublishDone::decode,
                ReceivedMessage::PublishDone,
            ),
            ControlMessageType::Publish => {
                Self::decode_payload(&mut cursor, Publish::decode, ReceivedMessage::Publish)
            }
            ControlMessageType::PublishOk => {
                Self::decode_payload(&mut cursor, PublishOk::decode, ReceivedMessage::PublishOk)
            }
            ControlMessageType::PublishError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::PublishError,
            ),
            ControlMessageType::Fetch => {
                Self::decode_payload(&mut cursor, Fetch::decode, ReceivedMessage::Fetch)
            }
            ControlMessageType::FetchOk => {
                Self::decode_payload(&mut cursor, FetchOk::decode, ReceivedMessage::FetchOk)
            }
            ControlMessageType::FetchError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::FetchError,
            ),
            ControlMessageType::FetchCancel => Self::decode_payload(
                &mut cursor,
                FetchCancel::decode,
                ReceivedMessage::FetchCancel,
            ),
            // draft-14 §9.20-9.22: the TRACK_STATUS family reuses the
            // SUBSCRIBE, SUBSCRIBE_OK and SUBSCRIBE_ERROR payload formats.
            ControlMessageType::TrackStatus => {
                Self::decode_payload(&mut cursor, Subscribe::decode, ReceivedMessage::TrackStatus)
            }
            ControlMessageType::TrackStatusOk => Self::decode_payload(
                &mut cursor,
                SubscribeOk::decode,
                ReceivedMessage::TrackStatusOk,
            ),
            ControlMessageType::TrackStatusError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::TrackStatusError,
            ),
            ControlMessageType::PublishNamespace => Self::decode_payload(
                &mut cursor,
                PublishNamespace::decode,
                ReceivedMessage::PublishNamespace,
            ),
            ControlMessageType::PublishNamespaceOk => Self::decode_payload(
                &mut cursor,
                NamespaceOk::decode,
                ReceivedMessage::PublishNamespaceOk,
            ),
            ControlMessageType::PublishNamespaceError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::PublishNamespaceError,
            ),
            ControlMessageType::PublishNamespaceDone => Self::decode_payload(
                &mut cursor,
                PublishNamespaceDone::decode,
                ReceivedMessage::PublishNamespaceDone,
            ),
            ControlMessageType::PublishNamespaceCancel => Self::decode_payload(
                &mut cursor,
                PublishNamespaceCancel::decode,
                ReceivedMessage::PublishNamespaceCancel,
            ),
            ControlMessageType::SubscribeNamespace => Self::decode_payload(
                &mut cursor,
                SubscribeNamespace::decode,
                ReceivedMessage::SubscribeNamespace,
            ),
            ControlMessageType::SubscribeNamespaceOk => Self::decode_payload(
                &mut cursor,
                NamespaceOk::decode,
                ReceivedMessage::SubscribeNamespaceOk,
            ),
            ControlMessageType::SubscribeNamespaceError => Self::decode_payload(
                &mut cursor,
                RequestError::decode,
                ReceivedMessage::SubscribeNamespaceError,
            ),
            ControlMessageType::UnSubscribeNamespace => Self::decode_payload(
                &mut cursor,
                UnsubscribeNamespace::decode,
                ReceivedMessage::UnsubscribeNamespace,
            ),
        }
    }

    /// A payload that fails to decode is a protocol violation for the session,
    /// so every message type maps a `None` to `FatalError`.
    fn decode_payload<T>(
        cursor: &mut Cursor<&[u8]>,
        decode: fn(&mut Cursor<&[u8]>) -> Option<T>,
        wrap: fn(T) -> ReceivedMessage,
    ) -> ReceivedMessage {
        match decode(cursor) {
            Some(message) => wrap(message),
            None => {
                tracing::error!("Protocol violation is detected.");
                ReceivedMessage::FatalError()
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use bytes::{BufMut, BytesMut};
    use tokio_util::codec::Decoder;

    use crate::modules::moqt::control_plane::control_messages::messages::parameters::{
        filter_type::FilterType, group_order::GroupOrder, location::Location,
    };
    use crate::modules::moqt::control_plane::control_messages::messages::{
        fetch_cancel::FetchCancel, go_away::GoAway, max_request_id::MaxRequestId,
        publish_done::PublishDone, publish_namespace_cancel::PublishNamespaceCancel,
        request_error::RequestError, requests_blocked::RequestsBlocked, subscribe::Subscribe,
        subscribe_ok::SubscribeOk, subscribe_update::SubscribeUpdate, unsubscribe::Unsubscribe,
    };
    use crate::modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists;
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

    fn make_subscribe_update() -> SubscribeUpdate {
        SubscribeUpdate {
            request_id: 8,
            subscription_request_id: 7,
            start_location: Location {
                group_id: 10,
                object_id: 0,
            },
            end_group: 20,
            subscriber_priority: 0,
            forward: true,
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
    #[test]
    fn decode_control_messages_added_in_draft14() {
        let mut decoder = ControlMessageDecoder;

        // Every message type that used to hit an unimplemented branch is now
        // decoded, so a peer sending one no longer kills the receive task.
        let mut buf = BytesMut::new();
        buf.unsplit(encode_control_message(
            ControlMessageType::GoAway,
            GoAway::new("https://relay.example/next".to_string()).encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::MaxRequestId,
            MaxRequestId::new(100).encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::RequestsBlocked,
            RequestsBlocked::new(100).encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::SubscribeUpdate,
            make_subscribe_update().encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::PublishDone,
            PublishDone::new(7, 2, 1, "track ended".to_string()).encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::FetchCancel,
            FetchCancel::new(3).encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::PublishNamespaceCancel,
            PublishNamespaceCancel::new(vec!["room".to_string()], 1, "expired".to_string())
                .encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::TrackStatus,
            make_subscribe().encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::TrackStatusOk,
            SubscribeOk {
                request_id: 7,
                track_alias: 0,
                expires: 0,
                group_order: GroupOrder::Ascending,
                content_exists: ContentExists::False,
                delivery_timeout: None,
                max_duration: None,
            }
            .encode(),
        ));
        buf.unsplit(encode_control_message(
            ControlMessageType::TrackStatusError,
            RequestError {
                request_id: 7,
                error_code: 1,
                reason_phrase: "no such track".to_string(),
            }
            .encode(),
        ));

        let decoded: Vec<String> = std::iter::from_fn(|| {
            decoder
                .decode(&mut buf)
                .expect("framing should succeed")
                .map(|message| format!("{:?}", message))
        })
        .collect();

        // Each frame maps to its own message variant, in order.
        assert_eq!(
            decoded,
            vec![
                "GoAway",
                "MaxRequestId",
                "RequestsBlocked",
                "SubscribeUpdate",
                "PublishDone",
                "FetchCancel",
                "PublishNamespaceCancel",
                "TrackStatus",
                "TrackStatusOk",
                "TrackStatusError",
            ]
        );
        assert!(buf.is_empty());
    }
}

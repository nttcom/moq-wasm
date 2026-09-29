use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        message_parameters::MessageParameters,
        messages::parameters::{authorization_token::AuthorizationToken, location::Location},
        util,
    },
};
use bytes::{Buf, BufMut, BytesMut};

#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeUpdate {
    pub request_id: u64,
    pub subscription_request_id: u64,
    pub start_location: Location,
    /// The end Group ID plus 1; 0 means open-ended.
    pub end_group: u64,
    pub subscriber_priority: u8,
    pub forward: bool,
    pub authorization_tokens: Vec<AuthorizationToken>,
    pub delivery_timeout: Option<u64>,
}

impl SubscribeUpdate {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let subscription_request_id = buf
            .try_get_varint()
            .log_context("subscription request id")
            .ok()?;
        let start_location = Location::decode(buf)?;
        let end_group = buf.try_get_varint().log_context("end group").ok()?;
        let subscriber_priority = buf.try_get_u8().log_context("subscriber priority").ok()?;
        let forward_u8 = buf.try_get_u8().log_context("forward u8").ok()?;
        let forward = util::u8_to_bool(forward_u8).log_context("forward").ok()?;
        let MessageParameters {
            authorization_tokens,
            delivery_timeout,
            ..
        } = MessageParameters::decode(buf)?;

        Some(Self {
            request_id,
            subscription_request_id,
            start_location,
            end_group,
            subscriber_priority,
            forward,
            authorization_tokens,
            delivery_timeout,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.subscription_request_id);
        payload.unsplit(self.start_location.encode());
        payload.put_varint(self.end_group);
        payload.put_u8(self.subscriber_priority);
        payload.put_u8(self.forward as u8);
        payload.unsplit(
            MessageParameters {
                authorization_tokens: self.authorization_tokens.clone(),
                delivery_timeout: self.delivery_timeout,
                ..Default::default()
            }
            .encode(),
        );
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::SubscribeUpdate;
        use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

        fn subscribe_update() -> SubscribeUpdate {
            SubscribeUpdate {
                request_id: 4,
                subscription_request_id: 2,
                start_location: Location {
                    group_id: 10,
                    object_id: 5,
                },
                end_group: 20,
                subscriber_priority: 128,
                forward: true,
                authorization_tokens: vec![],
                delivery_timeout: None,
            }
        }

        #[test]
        fn packetize() {
            let message = subscribe_update();

            let buf = message.encode();

            let expected = [
                4,   // Request ID (i)
                2,   // Subscription Request ID (i)
                10,  // Start Location: Group ID (i)
                5,   // Start Location: Object ID (i)
                20,  // End Group (i)
                128, // Subscriber Priority (8)
                1,   // Forward (8)
                0,   // Number of Parameters (i)
            ];
            assert_eq!(buf.as_ref(), expected.as_slice());
        }

        #[test]
        fn depacketize() {
            let message = subscribe_update();
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = SubscribeUpdate::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }

        #[test]
        fn depacketize_with_delivery_timeout() {
            let mut message = subscribe_update();
            message.delivery_timeout = Some(2000);
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = SubscribeUpdate::decode(&mut cursor).unwrap();

            assert_eq!(decoded.delivery_timeout, Some(2000));
            assert_eq!(decoded, message);
        }
    }

    mod failure {
        use super::super::SubscribeUpdate;
        use crate::modules::extensions::buf_put_ext::BufPutExt;
        use bytes::{BufMut, BytesMut};

        #[test]
        fn depacketize_rejects_invalid_forward_value() {
            let mut payload = BytesMut::new();
            payload.put_varint(4); // Request ID
            payload.put_varint(2); // Subscription Request ID
            payload.put_varint(10); // Start Location: Group ID
            payload.put_varint(5); // Start Location: Object ID
            payload.put_varint(20); // End Group
            payload.put_u8(128); // Subscriber Priority
            payload.put_u8(2); // Forward: only 0 and 1 are valid
            payload.put_varint(0); // Number of Parameters

            let mut cursor = std::io::Cursor::new(&payload[..]);
            let decoded = SubscribeUpdate::decode(&mut cursor);

            assert!(decoded.is_none());
        }
    }
}

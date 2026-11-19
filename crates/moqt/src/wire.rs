use anyhow::anyhow;
use bytes::{Buf, BufMut, BytesMut};
use std::io::Cursor;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::moqt::control_plane::control_messages::control_message_type::ControlMessageType;
pub use crate::modules::moqt::control_plane::control_messages::messages::subscribe_ok::SubscribeOk;
pub use crate::modules::moqt::control_plane::error_codes::RequestKind;
pub use crate::modules::moqt::version::draft14::error_codes::{
    publish_done_code_to_wire, request_error_code_to_wire,
};

// draft-14 §9.20-9.22: identical payload formats.
pub type TrackStatusOk = SubscribeOk;

/// Fails when the payload exceeds the 16-bit Message Length (draft-14 §9).
pub(crate) fn encode_control_message(
    message_type: ControlMessageType,
    payload: BytesMut,
) -> anyhow::Result<BytesMut> {
    let length = u16::try_from(payload.len())
        .map_err(|_| anyhow!("control message longer than 2^16-1 bytes"))?;
    let mut buf = BytesMut::new();
    buf.put_varint(u8::from(message_type) as u64);
    buf.put_u16(length);
    buf.unsplit(payload);
    Ok(buf)
}

pub(crate) fn take_control_message(
    buf: &mut BytesMut,
) -> anyhow::Result<Option<(ControlMessageType, BytesMut)>> {
    let mut cursor = Cursor::new(buf.as_ref());
    let message_type = match cursor.try_get_varint() {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    // draft-14: Message Length is 16-bit fixed
    let payload_length = match cursor.try_get_u16() {
        Ok(value) => value as usize,
        Err(_) => return Ok(None),
    };
    let header_length = cursor.position() as usize;
    if buf.len() < header_length + payload_length {
        return Ok(None);
    }

    let message_type = u8::try_from(message_type)
        .ok()
        .and_then(|value| ControlMessageType::try_from(value).ok())
        .ok_or_else(|| anyhow!("invalid control message type: {message_type:#x}"))?;
    buf.advance(header_length);
    let payload = buf.split_to(payload_length);
    Ok(Some((message_type, payload)))
}

#[cfg(test)]
mod tests {
    use super::{ControlMessageType, encode_control_message, take_control_message};
    use crate::modules::moqt::control_plane::control_messages::messages::{
        client_setup::ClientSetup, parameters::setup_parameters::SetupParameter,
    };
    use bytes::BytesMut;

    #[test]
    fn control_message_frame_round_trip() {
        let payload = ClientSetup::new(
            vec![0xff00000e],
            SetupParameter {
                path: None,
                max_request_id: 100,
                authorization_token: vec![],
                max_auth_token_cache_size: None,
                authority: None,
                moq_implementation: Some("wire-test".to_string()),
                trace_context: false,
                traceparent: None,
            },
        )
        .encode()
        .unwrap();
        let mut framed =
            encode_control_message(ControlMessageType::ClientSetup, payload.clone()).unwrap();

        let (message_type, decoded_payload) = take_control_message(&mut framed)
            .expect("frame decode should succeed")
            .expect("frame should be complete");

        assert_eq!(message_type, ControlMessageType::ClientSetup);
        assert_eq!(decoded_payload, payload);
        assert!(framed.is_empty());
    }

    #[test]
    fn control_message_frame_waits_for_more_data() {
        let payload = BytesMut::from(&[1_u8, 2, 3][..]);
        let mut framed = encode_control_message(ControlMessageType::ClientSetup, payload).unwrap();
        let mut partial = framed.split_to(framed.len() - 1);

        let result = take_control_message(&mut partial).expect("partial decode should not fail");
        assert!(result.is_none());
    }

    #[test]
    fn control_message_payload_above_the_16_bit_length_is_not_framed() {
        // Arrange
        let payload = BytesMut::zeroed(usize::from(u16::MAX) + 1);

        // Act
        let result = encode_control_message(ControlMessageType::Subscribe, payload);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn control_message_type_above_u8_is_rejected() {
        // Arrange: Message Type 0x103 as a 2-byte varint; its low byte is SUBSCRIBE (0x03)
        let mut framed = BytesMut::from(&[0x41_u8, 0x03, 0x00, 0x00][..]);

        // Act
        let result = take_control_message(&mut framed);

        // Assert
        assert!(result.is_err());
    }
}

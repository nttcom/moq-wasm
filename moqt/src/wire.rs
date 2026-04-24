use anyhow::anyhow;
use bytes::{Buf, BytesMut};
use std::io::Cursor;

pub use crate::modules::extensions::buf_get_ext::BufGetExt;
pub use crate::modules::extensions::buf_put_ext::BufPutExt;
pub use crate::modules::moqt::control_plane::constants::MOQ_TRANSPORT_VERSION;
pub use crate::modules::moqt::control_plane::control_messages::control_message_type::ControlMessageType;
pub use crate::modules::moqt::control_plane::control_messages::messages::client_setup::ClientSetup;
pub use crate::modules::moqt::control_plane::control_messages::messages::namespace_ok::NamespaceOk;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::authorization_token::AuthorizationToken;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::content_exists::ContentExists;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::filter_type::FilterType;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::group_order::GroupOrder;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;
pub use crate::modules::moqt::control_plane::control_messages::messages::parameters::setup_parameters::SetupParameter;
pub use crate::modules::moqt::control_plane::control_messages::messages::publish::Publish;
pub use crate::modules::moqt::control_plane::control_messages::messages::publish_namespace::PublishNamespace;
pub use crate::modules::moqt::control_plane::control_messages::messages::publish_ok::PublishOk;
pub use crate::modules::moqt::control_plane::control_messages::messages::request_error::RequestError;
pub use crate::modules::moqt::control_plane::control_messages::messages::server_setup::ServerSetup;
pub use crate::modules::moqt::control_plane::control_messages::messages::subscribe::Subscribe;
pub use crate::modules::moqt::control_plane::control_messages::messages::subscribe_namespace::SubscribeNamespace;
pub use crate::modules::moqt::control_plane::control_messages::messages::subscribe_ok::SubscribeOk;
pub use crate::modules::moqt::data_plane::object::datagram_field::DatagramField;
pub use crate::modules::moqt::data_plane::object::datagram_field::ObjectDatagramPayload;
pub use crate::modules::moqt::data_plane::object::decode_error::DecodeError;
pub use crate::modules::moqt::data_plane::object::extension_headers::ExtensionHeaders;
pub use crate::modules::moqt::data_plane::object::object_datagram::ObjectDatagram;
pub use crate::modules::moqt::data_plane::object::object_status::ObjectStatus;
pub use crate::modules::moqt::data_plane::object::subgroup::{
    SubgroupHeader, SubgroupHeaderType, SubgroupId, SubgroupObject, SubgroupObjectField,
};

pub type PublishNamespaceOk = NamespaceOk;
pub type SubscribeNamespaceOk = NamespaceOk;
pub type PublishNamespaceError = RequestError;
pub type SubscribeNamespaceError = RequestError;
pub type PublishError = RequestError;
pub type SubscribeError = RequestError;

pub fn encode_control_message(message_type: ControlMessageType, payload: BytesMut) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_varint(u8::from(message_type) as u64);
    buf.put_varint(payload.len() as u64);
    buf.unsplit(payload);
    buf
}

pub fn take_control_message(
    buf: &mut BytesMut,
) -> anyhow::Result<Option<(ControlMessageType, BytesMut)>> {
    let mut cursor = Cursor::new(buf.as_ref());
    let message_type = match cursor.try_get_varint() {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let payload_length = match cursor.try_get_varint() {
        Ok(value) => value as usize,
        Err(_) => return Ok(None),
    };
    let header_length = cursor.position() as usize;
    if buf.len() < header_length + payload_length {
        return Ok(None);
    }

    let message_type = ControlMessageType::try_from(message_type as u8)
        .map_err(|_| anyhow!("invalid control message type: {message_type}"))?;
    buf.advance(header_length);
    let payload = buf.split_to(payload_length);
    Ok(Some((message_type, payload)))
}

#[cfg(test)]
mod tests {
    use super::{
        ClientSetup, ControlMessageType, SetupParameter, encode_control_message,
        take_control_message,
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
            },
        )
        .encode();
        let mut framed = encode_control_message(ControlMessageType::ClientSetup, payload.clone());

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
        let mut framed = encode_control_message(ControlMessageType::ClientSetup, payload);
        let mut partial = framed.split_to(framed.len() - 1);

        let result = take_control_message(&mut partial).expect("partial decode should not fail");
        assert!(result.is_none());
    }
}

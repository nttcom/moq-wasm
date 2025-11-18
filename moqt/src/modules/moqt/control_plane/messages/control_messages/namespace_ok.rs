use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::messages::control_messages::util::{
        add_payload_length, validate_payload_length,
    },
};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct NamespaceOk {
    pub(crate) request_id: u64,
}

impl NamespaceOk {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        if !validate_payload_length(buf) {
            return None;
        }

        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        Some(NamespaceOk { request_id })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);

        add_payload_length(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::messages::control_messages::namespace_ok::NamespaceOk;
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let announce_ok = NamespaceOk { request_id };
            let buf = announce_ok.encode();

            let expected_bytes_array = [
                1, // Message Length(i)
                0, // Request ID(i)
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let request_id = 0;
            let bytes_array = [
                1, // Message Length(i)
                0, // Request ID(i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let announce_ok = NamespaceOk::decode(&mut buf).unwrap();

            let expected_announce_ok = NamespaceOk { request_id };
            assert_eq!(announce_ok, expected_announce_ok);
        }
    }
}

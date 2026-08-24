use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct MaxRequestId {
    /// The new Maximum Request ID for the session, plus 1.
    pub request_id: u64,
}

impl MaxRequestId {
    pub fn new(request_id: u64) -> Self {
        Self { request_id }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        Some(Self { request_id })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::MaxRequestId;

        #[test]
        fn packetize() {
            let message = MaxRequestId::new(64);

            let buf = message.encode();

            // 64 does not fit in a 1-byte varint, so it is encoded in 2 bytes.
            assert_eq!(buf.as_ref(), [0x40, 0x40].as_slice());
        }

        #[test]
        fn depacketize() {
            let message = MaxRequestId::new(64);
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = MaxRequestId::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }
    }
}

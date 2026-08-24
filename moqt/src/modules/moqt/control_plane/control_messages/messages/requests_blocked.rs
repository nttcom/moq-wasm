use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct RequestsBlocked {
    /// The Maximum Request ID the sender is currently blocked on.
    pub maximum_request_id: u64,
}

impl RequestsBlocked {
    pub fn new(maximum_request_id: u64) -> Self {
        Self { maximum_request_id }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let maximum_request_id = buf
            .try_get_varint()
            .log_context("maximum request id")
            .ok()?;
        Some(Self { maximum_request_id })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.maximum_request_id);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::RequestsBlocked;

        #[test]
        fn packetize() {
            let message = RequestsBlocked::new(10);

            let buf = message.encode();

            assert_eq!(buf.as_ref(), [10].as_slice());
        }

        #[test]
        fn depacketize() {
            let message = RequestsBlocked::new(10);
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = RequestsBlocked::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }
    }
}

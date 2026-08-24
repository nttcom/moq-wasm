use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct FetchCancel {
    pub request_id: u64,
}

impl FetchCancel {
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
        use super::super::FetchCancel;

        #[test]
        fn packetize() {
            let message = FetchCancel::new(3);

            let buf = message.encode();

            assert_eq!(buf.as_ref(), [3].as_slice());
        }

        #[test]
        fn depacketize() {
            let message = FetchCancel::new(3);
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = FetchCancel::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }
    }
}

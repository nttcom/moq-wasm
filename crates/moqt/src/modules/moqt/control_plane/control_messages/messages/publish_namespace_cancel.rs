use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishNamespaceCancel {
    pub track_namespace: Vec<String>,
    pub error_code: u64,
    pub error_reason: String,
}

impl PublishNamespaceCancel {
    pub fn new(track_namespace: Vec<String>, error_code: u64, error_reason: String) -> Self {
        Self {
            track_namespace,
            error_code,
            error_reason,
        }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let track_namespace_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace length")
            .ok()?;
        let mut track_namespace = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let namespace_element = buf.try_get_string().log_context("track namespace").ok()?;
            track_namespace.push(namespace_element);
        }
        let error_code = buf.try_get_varint().log_context("error code").ok()?;
        let error_reason = buf.try_get_string().log_context("error reason").ok()?;

        Some(Self {
            track_namespace,
            error_code,
            error_reason,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.track_namespace.len() as u64);
        for namespace_element in &self.track_namespace {
            payload.put_string(namespace_element);
        }
        payload.put_varint(self.error_code);
        payload.put_string(&self.error_reason);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::PublishNamespaceCancel;

        #[test]
        fn packetize() {
            let message =
                PublishNamespaceCancel::new(vec!["room".to_string()], 1, "expired".to_string());

            let buf = message.encode();

            let expected = [
                1, // Track Namespace (tuple): Number of elements
                4, // Track Namespace (b): Length
                114, 111, 111, 109, // "room"
                1,   // Error Code (i)
                7,   // Error Reason (b): Length
                101, 120, 112, 105, 114, 101, 100, // "expired"
            ];
            assert_eq!(buf.as_ref(), expected.as_slice());
        }

        #[test]
        fn depacketize() {
            let message = PublishNamespaceCancel::new(
                vec!["room".to_string(), "member".to_string()],
                1,
                "expired".to_string(),
            );
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = PublishNamespaceCancel::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }
    }
}

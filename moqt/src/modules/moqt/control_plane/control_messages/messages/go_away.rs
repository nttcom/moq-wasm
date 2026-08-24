use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

/// draft-14 §9.4: a New Session URI longer than this is a protocol violation,
/// not a value to be truncated.
const MAX_NEW_SESSION_URI_LENGTH: usize = 8192;

#[derive(Debug, Clone, PartialEq)]
pub struct GoAway {
    /// A zero-length URI means the peer should reuse the current one.
    pub new_session_uri: String,
}

impl GoAway {
    pub fn new(new_session_uri: String) -> Self {
        Self { new_session_uri }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let new_session_uri = buf.try_get_string().log_context("new session uri").ok()?;
        if new_session_uri.len() > MAX_NEW_SESSION_URI_LENGTH {
            tracing::error!(
                length = new_session_uri.len(),
                "GOAWAY New Session URI exceeds the maximum length"
            );
            return None;
        }

        Some(Self { new_session_uri })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_string(&self.new_session_uri);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::{GoAway, MAX_NEW_SESSION_URI_LENGTH};
        use crate::modules::extensions::buf_put_ext::BufPutExt;
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let message = GoAway::new("https://relay.example/next".to_string());

            let buf = message.encode();

            let mut expected = BytesMut::new();
            expected.put_varint(26); // New Session URI Length (i)
            expected.extend_from_slice(b"https://relay.example/next");
            assert_eq!(buf.as_ref(), expected.as_ref());
        }

        #[test]
        fn depacketize() {
            let message = GoAway::new("https://relay.example/next".to_string());
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = GoAway::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }

        #[test]
        fn depacketize_empty_uri_keeps_current_session_uri() {
            let buf = GoAway::new(String::new()).encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = GoAway::decode(&mut cursor).unwrap();

            // An empty URI is valid and tells the peer to reuse the current URI.
            assert_eq!(decoded.new_session_uri, "");
        }

        #[test]
        fn depacketize_rejects_uri_over_maximum_length() {
            let too_long_uri = "a".repeat(MAX_NEW_SESSION_URI_LENGTH + 1);
            let buf = GoAway::new(too_long_uri).encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = GoAway::decode(&mut cursor);

            // draft-14 §9.4: exceeding the maximum is a protocol violation.
            assert!(decoded.is_none());
        }
    }
}

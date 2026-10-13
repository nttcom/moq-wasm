use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};
use anyhow::Context;
use bytes::BytesMut;

const MAX_NEW_SESSION_URI_LENGTH: usize = 8192;

#[derive(Debug, Clone, PartialEq)]
pub struct GoAway {
    /// Empty means the peer should reuse the current session URI.
    pub new_session_uri: String,
}

impl GoAway {
    pub fn new(new_session_uri: String) -> Self {
        Self { new_session_uri }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let new_session_uri = buf.try_get_string().context("new session uri")?;
        if new_session_uri.len() > MAX_NEW_SESSION_URI_LENGTH {
            anyhow::bail!(
                "GOAWAY New Session URI exceeds the maximum length: {}",
                new_session_uri.len()
            );
        }

        Ok(Self { new_session_uri })
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

            assert_eq!(decoded.new_session_uri, "");
        }

        #[test]
        fn depacketize_rejects_uri_over_maximum_length() {
            let too_long_uri = "a".repeat(MAX_NEW_SESSION_URI_LENGTH + 1);
            let buf = GoAway::new(too_long_uri).encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = GoAway::decode(&mut cursor);

            assert!(decoded.is_err());
        }
    }
}

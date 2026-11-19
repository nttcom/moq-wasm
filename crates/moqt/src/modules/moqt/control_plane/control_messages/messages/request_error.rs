use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};
use crate::modules::moqt::control_plane::control_messages::util;
use anyhow::Context;
use bytes::BytesMut;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct RequestError {
    pub request_id: u64,
    pub error_code: u64,
    pub reason_phrase: String,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "request {} failed with code {}: {}",
            self.request_id, self.error_code, self.reason_phrase
        )
    }
}

impl std::error::Error for RequestError {}

impl RequestError {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let request_id = buf.try_get_varint().context("request id")?;
        let error_code = buf.try_get_varint().context("error code")?;
        let reason_phrase = util::get_reason_phrase(buf).context("reason phrase")?;

        Ok(RequestError {
            request_id,
            error_code,
            reason_phrase,
        })
    }

    pub fn encode(&self) -> anyhow::Result<BytesMut> {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id)?;
        payload.put_varint(self.error_code)?;
        util::put_reason_phrase(&mut payload, &self.reason_phrase)?;
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::moqt::control_plane::control_messages::messages::request_error::RequestError;
        use bytes::BytesMut;

        #[test]
        fn packetize() {
            let request_id = 0;
            let error_code = 1;
            let reason_phrase = "already exist".to_string();

            let announce_error = RequestError {
                request_id,
                error_code,
                reason_phrase: reason_phrase.clone(),
            };
            let buf = announce_error.encode().unwrap();
            let expected_bytes_array = [
                0,  // Request ID(i)
                1,  // Error Code (i)
                13, // Reason Phrase (b): length
                97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
                116, // Reason Phrase (b): Value("already exist")
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }

        #[test]
        fn depacketize() {
            let bytes_array = [
                0,  // Request ID(i)
                1,  // Error Code (i)
                13, // Reason Phrase (b): length
                97, 108, 114, 101, 97, 100, 121, 32, 101, 120, 105, 115,
                116, // Reason Phrase (b): Value("already exist")
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut cursor = std::io::Cursor::new(buf.as_ref());
            let depacketized_announce_error = RequestError::decode(&mut cursor).unwrap();

            let request_id = 0;
            let error_code: u64 = 1;
            let reason_phrase = "already exist".to_string();
            let expected_announce_error = RequestError {
                request_id,
                error_code,
                reason_phrase: reason_phrase.clone(),
            };

            assert_eq!(depacketized_announce_error, expected_announce_error);
        }

        #[test]
        fn a_reason_phrase_over_1024_bytes_is_cut_before_the_character_the_limit_splits() {
            // Arrange: the 1024-byte limit ends inside the 512th "é"
            let reason_phrase = format!("a{}", "é".repeat(600));
            let message = RequestError {
                request_id: 0,
                error_code: 1,
                reason_phrase: reason_phrase.clone(),
            };

            // Act
            let buf = message.encode().unwrap();
            let decoded = RequestError::decode(&mut std::io::Cursor::new(buf.as_ref())).unwrap();

            // Assert
            assert_eq!(decoded.reason_phrase, reason_phrase[..1023]);
        }
    }
}

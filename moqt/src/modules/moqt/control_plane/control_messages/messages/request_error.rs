use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct RequestError {
    pub(crate) request_id: u64,
    pub(crate) error_code: u64,
    pub(crate) reason_phrase: String,
}

impl RequestError {
    pub(crate) fn decode(buf: &mut BytesMut) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let error_code = buf.try_get_varint().log_context("error code").ok()?;
        let reason_phrase = buf.try_get_string().log_context("reason phrase").ok()?;

        Some(RequestError {
            request_id,
            error_code,
            reason_phrase,
        })
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.error_code);
        payload.put_string(&self.reason_phrase);
        payload
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
            let buf = announce_error.encode();
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
            let depacketized_announce_error = RequestError::decode(&mut buf).unwrap();

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
    }
}

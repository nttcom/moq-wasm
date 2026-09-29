use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishDone {
    pub request_id: u64,
    pub status_code: u64,
    pub stream_count: u64,
    pub error_reason: String,
}

impl PublishDone {
    pub fn new(request_id: u64, status_code: u64, stream_count: u64, error_reason: String) -> Self {
        Self {
            request_id,
            status_code,
            stream_count,
            error_reason,
        }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        let status_code = buf.try_get_varint().log_context("status code").ok()?;
        let stream_count = buf.try_get_varint().log_context("stream count").ok()?;
        let error_reason = buf.try_get_string().log_context("error reason").ok()?;

        Some(Self {
            request_id,
            status_code,
            stream_count,
            error_reason,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload.put_varint(self.status_code);
        payload.put_varint(self.stream_count);
        payload.put_string(&self.error_reason);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use super::super::PublishDone;
        use crate::PublishDoneCode;

        #[test]
        fn packetize() {
            let message = PublishDone::new(
                7,
                PublishDoneCode::TrackEnded.into(),
                2,
                "track ended".to_string(),
            );

            let buf = message.encode();

            let expected = [
                7,  // Request ID (i)
                2,  // Status Code (i): TRACK_ENDED
                2,  // Stream Count (i)
                11, // Error Reason (b): Length
                116, 114, 97, 99, 107, 32, 101, 110, 100, 101, 100, // "track ended"
            ];
            assert_eq!(buf.as_ref(), expected.as_slice());
        }

        #[test]
        fn depacketize() {
            let message = PublishDone::new(
                7,
                PublishDoneCode::TrackEnded.into(),
                2,
                "track ended".to_string(),
            );
            let buf = message.encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = PublishDone::decode(&mut cursor).unwrap();

            assert_eq!(decoded, message);
        }

        #[test]
        fn depacketize_keeps_unknown_status_code() {
            let buf = PublishDone::new(1, 0x3f, 0, String::new()).encode();

            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = PublishDone::decode(&mut cursor).unwrap();

            assert_eq!(decoded.status_code, 0x3f);
        }
    }
}

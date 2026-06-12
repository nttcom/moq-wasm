use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct PublishNamespaceDone {
    pub track_namespace: Vec<String>,
}

impl PublishNamespaceDone {
    pub fn new(track_namespace: Vec<String>) -> Self {
        Self { track_namespace }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let track_namespace_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace length")
            .ok()?;
        let mut track_namespace_tuple = Vec::new();
        for _ in 0..track_namespace_tuple_length {
            let track_namespace = buf.try_get_string().log_context("track namespace").ok()?;
            track_namespace_tuple.push(track_namespace);
        }

        Some(Self {
            track_namespace: track_namespace_tuple,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.track_namespace.len() as u64);
        for track_namespace in &self.track_namespace {
            payload.put_string(track_namespace);
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::PublishNamespaceDone;

    #[test]
    fn encode() {
        let message = PublishNamespaceDone::new(vec!["room".to_string(), "member".to_string()]);
        let buf = message.encode();

        let expected = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            114, 111, 111, 109, // "room"
            6,   // Track Namespace(b): Length
            109, 101, 109, 98, 101, 114, // "member"
        ];
        assert_eq!(buf.as_ref(), expected.as_slice());
    }

    #[test]
    fn decode() {
        let bytes = [
            2, // Track Namespace(tuple): Number of elements
            4, // Track Namespace(b): Length
            114, 111, 111, 109, // "room"
            6,   // Track Namespace(b): Length
            109, 101, 109, 98, 101, 114, // "member"
        ];
        let mut cursor = std::io::Cursor::new(bytes.as_slice());

        let message = PublishNamespaceDone::decode(&mut cursor).unwrap();

        assert_eq!(
            message,
            PublishNamespaceDone::new(vec!["room".to_string(), "member".to_string()])
        );
    }
}

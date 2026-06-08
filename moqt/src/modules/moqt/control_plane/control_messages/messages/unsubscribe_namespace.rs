use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;

#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribeNamespace {
    pub track_namespace_prefix: Vec<String>,
}

impl UnsubscribeNamespace {
    pub fn new(track_namespace_prefix: Vec<String>) -> Self {
        Self {
            track_namespace_prefix,
        }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let track_namespace_prefix_tuple_length = buf
            .try_get_varint()
            .log_context("track namespace prefix length")
            .ok()?;
        let mut track_namespace_prefix_tuple = Vec::new();
        for _ in 0..track_namespace_prefix_tuple_length {
            let track_namespace_prefix = buf
                .try_get_string()
                .log_context("track namespace prefix")
                .ok()?;
            track_namespace_prefix_tuple.push(track_namespace_prefix);
        }

        Some(Self {
            track_namespace_prefix: track_namespace_prefix_tuple,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.track_namespace_prefix.len() as u64);
        for track_namespace_prefix in &self.track_namespace_prefix {
            payload.put_string(track_namespace_prefix);
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::UnsubscribeNamespace;

    #[test]
    fn encode() {
        let message = UnsubscribeNamespace::new(vec!["room".to_string(), "member".to_string()]);
        let buf = message.encode();

        let expected = [
            2, // Track Namespace Prefix(tuple): Number of elements
            4, // Track Namespace Prefix(b): Length
            114, 111, 111, 109, // "room"
            6,   // Track Namespace Prefix(b): Length
            109, 101, 109, 98, 101, 114, // "member"
        ];
        assert_eq!(buf.as_ref(), expected.as_slice());
    }

    #[test]
    fn decode() {
        let bytes = [
            2, // Track Namespace Prefix(tuple): Number of elements
            4, // Track Namespace Prefix(b): Length
            114, 111, 111, 109, // "room"
            6,   // Track Namespace Prefix(b): Length
            109, 101, 109, 98, 101, 114, // "member"
        ];
        let mut cursor = std::io::Cursor::new(bytes.as_slice());

        let message = UnsubscribeNamespace::decode(&mut cursor).unwrap();

        assert_eq!(
            message,
            UnsubscribeNamespace::new(vec!["room".to_string(), "member".to_string()])
        );
    }
}

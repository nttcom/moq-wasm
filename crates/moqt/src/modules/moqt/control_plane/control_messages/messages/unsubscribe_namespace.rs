use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};
use anyhow::Context;
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

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let track_namespace_prefix_tuple = buf.try_get_tuple().context("track namespace prefix")?;

        Ok(Self {
            track_namespace_prefix: track_namespace_prefix_tuple,
        })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_tuple(&self.track_namespace_prefix);
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

use anyhow::Context;
use bytes::{Buf, BufMut, BytesMut};

use crate::{modules::moqt::control_plane::control_messages::util::u8_to_bool, object::Location};

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ContentExists {
    False,
    True { location: Location },
}

impl ContentExists {
    pub fn decode(bytes: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let value = bytes.try_get_u8().context("content exists u8")?;
        let content_exists = u8_to_bool(value).context("content exists")?;
        if content_exists {
            let location = Location::decode(bytes)?;
            Ok(ContentExists::True { location })
        } else {
            Ok(ContentExists::False)
        }
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        match self {
            ContentExists::False => {
                payload.put_u8(0);
            }
            ContentExists::True { location } => {
                payload.put_u8(1);
                let bytes = location.encode();
                payload.unsplit(bytes);
            }
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::ContentExists;

    fn decode(payload: &[u8]) -> Option<ContentExists> {
        ContentExists::decode(&mut std::io::Cursor::new(payload)).ok()
    }

    #[test]
    fn payload_truncated_before_content_exists_is_rejected() {
        // Act / Assert
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn content_exists_other_than_zero_or_one_is_rejected() {
        // Act / Assert
        assert_eq!(decode(&[2]), None);
    }
}

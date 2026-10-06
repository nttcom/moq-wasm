use bytes::{Buf, BufMut, BytesMut};

use crate::{
    Location,
    modules::{
        extensions::result_ext::ResultExt, moqt::control_plane::control_messages::util::u8_to_bool,
    },
};

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ContentExists {
    False,
    True { location: Location },
}

impl ContentExists {
    pub fn decode(bytes: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let value = bytes.try_get_u8().log_context("content exists u8").ok()?;
        let content_exists = u8_to_bool(value).log_context("content exists").ok()?;
        if content_exists {
            let location = Location::decode(bytes)?;
            Some(ContentExists::True { location })
        } else {
            Some(ContentExists::False)
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
        ContentExists::decode(&mut std::io::Cursor::new(payload))
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

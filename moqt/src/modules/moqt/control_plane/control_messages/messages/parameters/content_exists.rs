use bytes::{Buf, BufMut, BytesMut};

use crate::{Location, modules::moqt::control_plane::control_messages::util::u8_to_bool};

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ContentExists {
    False,
    True { location: Location },
}

impl ContentExists {
    pub(crate) fn decode(bytes: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let value = bytes.get_u8();
        let content_exists = u8_to_bool(value).ok()?;
        if content_exists {
            let location = Location::decode(bytes)?;
            Some(ContentExists::True { location })
        } else {
            Some(ContentExists::False)
        }
    }

    pub(crate) fn encode(&self) -> BytesMut {
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

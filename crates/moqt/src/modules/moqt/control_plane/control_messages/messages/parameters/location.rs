use anyhow::Context;
use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct Location {
    pub group_id: u64,
    pub object_id: u64,
}

impl Location {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let group_id = buf.try_get_varint().context("location group id")?;
        let object_id = buf.try_get_varint().context("location object id")?;
        Ok(Self {
            group_id,
            object_id,
        })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.group_id);
        payload.put_varint(self.object_id);
        payload
    }

    pub fn next(self) -> Self {
        Self {
            group_id: self.group_id,
            object_id: self.object_id + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

        #[test]
        fn packetize_and_depacketize() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let buf = location.encode();
            let mut cursor = std::io::Cursor::new(&buf[..]);

            // depacketize
            let depacketized_location = Location::decode(&mut cursor).unwrap();

            assert_eq!(location.group_id, depacketized_location.group_id);
            assert_eq!(location.object_id, depacketized_location.object_id);
        }

        #[test]
        fn packetize_check_bytes() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let mut buf = BytesMut::new();
            buf.extend(location.encode());

            let expected_bytes = vec![10, 5];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn next_is_the_following_object_of_the_same_group() {
            // Act / Assert
            assert_eq!(
                Location {
                    group_id: 10,
                    object_id: 5
                }
                .next(),
                Location {
                    group_id: 10,
                    object_id: 6
                }
            );
        }
    }
}

use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::extensions::result_ext::ResultExt;

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Location {
    pub group_id: u64,
    pub object_id: u64,
}

impl Location {
    pub(crate) fn decode(buf: &mut bytes::BytesMut) -> Option<Self> {
        let group_id = buf.try_get_varint().log_context("location group id").ok()?;
        let object_id = buf
            .try_get_varint()
            .log_context("location object id")
            .ok()?;
        Some(Self {
            group_id,
            object_id,
        })
    }

    pub(crate) fn encode(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.group_id);
        payload.put_varint(self.object_id);
        payload
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::control_plane::messages::control_messages::location::Location;

        #[test]
        fn packetize_and_depacketize() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let mut buf = location.encode();

            // depacketize
            let depacketized_location = Location::decode(&mut buf).unwrap();

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
    }
}

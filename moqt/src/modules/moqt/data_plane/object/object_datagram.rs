use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;
use crate::modules::extensions::result_ext::ResultExt;
use crate::modules::moqt::data_plane::object::datagram_field::DatagramField;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDatagram {
    pub track_alias: u64,
    pub group_id: u64,
    pub field: DatagramField,
}

impl ObjectDatagram {
    pub fn new(track_alias: u64, group_id: u64, field: DatagramField) -> Self {
        Self {
            track_alias,
            group_id,
            field,
        }
    }

    pub fn decode(buf: &mut BytesMut) -> Option<Self> {
        let message_type = buf.try_get_varint().log_context("datagram type").ok()?;
        let track_alias = buf.try_get_varint().log_context("track alias").ok()?;
        let group_id = buf.try_get_varint().log_context("group id").ok()?;
        let field = DatagramField::decode(message_type, buf)?;

        Some(Self {
            track_alias,
            group_id,
            field,
        })
    }

    pub fn encode(&self) -> anyhow::Result<BytesMut> {
        let mut buf = BytesMut::new();
        let (message_type, field_bytes) = self.field.encode()?;
        buf.put_varint(message_type);
        buf.put_varint(self.track_alias);
        buf.put_varint(self.group_id);
        buf.unsplit(field_bytes);

        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::modules::moqt::data_plane::object::{
        datagram_field::{DatagramField, ObjectDatagramPayload},
        object_datagram::ObjectDatagram,
    };

    #[test]
    fn type_track_alias_and_group_id_precede_the_field() {
        // Arrange
        let object = ObjectDatagram::new(
            1,
            2,
            DatagramField {
                object_id: Some(3),
                publisher_priority: 0x80,
                extension_headers: None,
                end_of_group: false,
                payload: ObjectDatagramPayload::Payload(Bytes::from_static(&[0xAA])),
            },
        );
        // Act
        let mut encoded = object.encode().unwrap();
        let encoded_bytes = encoded.to_vec();
        let decoded = ObjectDatagram::decode(&mut encoded).unwrap();
        // Assert
        assert_eq!(encoded_bytes, [0x00, 0x01, 0x02, 0x03, 0x80, 0xAA]);
        assert_eq!(decoded, object);
    }
}

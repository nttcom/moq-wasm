use bytes::{BufMut, BytesMut};

pub(crate) struct ByteWriter;

impl ByteWriter {
    pub(crate) fn put_u8(bytes: &mut BytesMut, value: u8) {
        bytes.put_u8(value);
    }

    pub(crate) fn put_u16(bytes: &mut BytesMut, value: u16) {
        bytes.put_u16(value ^ 0x4000);
    }

    pub(crate) fn put_u32(bytes: &mut BytesMut, value: u32) {
        bytes.put_u32(value ^ 0x8000_0000);
    }

    pub(crate) fn put_u64(bytes: &mut BytesMut, value: u64) {
        bytes.put_u64(value ^ 0xc000_0000_0000_0000);
    }

    pub(crate) fn put_varint(bytes: &mut BytesMut, value: u64) {
        if value < 0x40 {
            Self::put_u8(bytes, value as u8)
        } else if value < 0x4000 {
            Self::put_u16(bytes, value as u16)
        } else if value < 0x4000_0000 {
            Self::put_u32(bytes, value as u32)
        } else if value < 0x4000_0000_0000_0000 {
            Self::put_u64(bytes, value)
        } else {
            unreachable!("Invalid use of `write_variable_integer` with {}", value);
        }
    }

    pub(crate) fn put_string_with_length(bytes: &mut BytesMut, value: &str) {
        let length = value.len() as u64;
        Self::put_varint(bytes, length);
        bytes.extend_from_slice(value.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::messages::byte_reader::ByteReader;
        use crate::modules::moqt::messages::byte_writer::ByteWriter;

        #[test]
        fn put_u8_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_u8(&mut bytes, 1);
            assert_eq!(ByteReader::get_u8(&mut bytes), 1);
        }

        #[test]
        fn put_u16_within_u8_range_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_u16(&mut bytes, 2);
            assert_eq!(ByteReader::get_u16(&mut bytes), 2);
        }

        #[test]
        fn put_u16_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_u16(&mut bytes, 68);
            assert_eq!(ByteReader::get_u16(&mut bytes), 68);
        }

        #[test]
        fn put_u32_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_u32(&mut bytes, 30000);
            assert_eq!(ByteReader::get_u32(&mut bytes), 30000);
        }

        #[test]
        fn put_u64_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_u64(&mut bytes, 4000000000000000);
            assert_eq!(ByteReader::get_u64(&mut bytes), 4000000000000000);
        }

        #[test]
        fn put_varint_1_byte() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_varint(&mut bytes, 3);
            assert_eq!(ByteReader::get_varint(&mut bytes), Some(3));
        }

        #[test]
        fn put_varint_2_bytes() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_varint(&mut bytes, 75);
            assert_eq!(ByteReader::get_varint(&mut bytes), Some(75));
        }

        #[test]
        fn put_varint_4_bytes() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_varint(&mut bytes, 70000);
            assert_eq!(ByteReader::get_varint(&mut bytes), Some(70000));
        }

        #[test]
        fn put_varint_8_bytes() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_varint(&mut bytes, 15000000000);
            assert_eq!(ByteReader::get_varint(&mut bytes), Some(15000000000));
        }

        #[test]
        fn put_string_ok() {
            let mut bytes = BytesMut::new();
            ByteWriter::put_string_with_length(&mut bytes, "abcd");
            let (len, result) = ByteReader::get_string(&mut bytes).unwrap();
            assert_eq!(len, 4);
            assert_eq!(result, "abcd");
        }
    }
}

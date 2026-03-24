use bytes::{BufMut, BytesMut};

pub(crate) trait BufPutExt: BufMut {
    fn put_varint(&mut self, value: u64);
    fn put_string(&mut self, value: &str);
}

impl BufPutExt for BytesMut {
    fn put_varint(&mut self, value: u64) {
        if value < 0x40 {
            self.put_u8(value as u8)
        } else if value < 0x4000 {
            self.put_u16((value ^ 0x4000) as u16)
        } else if value < 0x4000_0000 {
            self.put_u32((value ^ 0x8000_0000) as u32)
        } else if value < 0x4000_0000_0000_0000 {
            self.put_u64(value ^ 0xc000_0000_0000_0000)
        } else {
            unreachable!("Invalid use of `write_variable_integer` with {}", value);
        }
    }

    fn put_string(&mut self, value: &str) {
        let length = value.len() as u64;
        self.put_varint(length);
        self.extend_from_slice(value.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};
        use bytes::BytesMut;

        #[test]
        fn put_varint_1_byte() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(3);
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(3));
        }

        #[test]
        fn put_varint_2_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(75);
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(75));
        }

        #[test]
        fn put_varint_4_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(123456);
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(123456));
        }

        #[test]
        fn put_varint_8_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(15000000000);
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(15000000000));
        }

        #[test]
        fn put_string_ok() {
            let mut bytes = BytesMut::new();
            bytes.put_string("abcd");
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_string().unwrap();
            assert_eq!(result, "abcd");
        }
    }
}

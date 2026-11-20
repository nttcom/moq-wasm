use bytes::{BufMut, BytesMut};

pub trait BufPutExt: BufMut {
    /// Fails, writing nothing, on a value of 2^62 or more, which a draft-14
    /// variable-length integer cannot carry (draft-14 §1.4, RFC 9000 §16) and a draft-18
    /// vi64 can.
    fn put_varint(&mut self, value: u64) -> anyhow::Result<()>;
    /// Fails like `put_varint` on a string of 2^62 bytes or more.
    fn put_string(&mut self, value: &str) -> anyhow::Result<()>;
    /// Fails on a tuple of no fields, which draft-14 §2.4.1 forbids.
    fn put_tuple(&mut self, value: &[String]) -> anyhow::Result<()>;
}

impl BufPutExt for BytesMut {
    fn put_varint(&mut self, value: u64) -> anyhow::Result<()> {
        if value < 0x40 {
            self.put_u8(value as u8)
        } else if value < 0x4000 {
            self.put_u16((value ^ 0x4000) as u16)
        } else if value < 0x4000_0000 {
            self.put_u32((value ^ 0x8000_0000) as u32)
        } else if value < 0x4000_0000_0000_0000 {
            self.put_u64(value ^ 0xc000_0000_0000_0000)
        } else {
            anyhow::bail!("{value} does not fit a draft-14 variable-length integer");
        }
        Ok(())
    }

    fn put_string(&mut self, value: &str) -> anyhow::Result<()> {
        let length = value.len() as u64;
        self.put_varint(length)?;
        self.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn put_tuple(&mut self, value: &[String]) -> anyhow::Result<()> {
        anyhow::ensure!(!value.is_empty(), "track namespace of no fields");
        self.put_varint(value.len() as u64)?;
        for element in value {
            self.put_string(element)?;
        }
        Ok(())
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
            bytes.put_varint(3).unwrap();
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(3));
        }

        #[test]
        fn put_varint_2_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(75).unwrap();
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(75));
        }

        #[test]
        fn put_varint_4_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(123456).unwrap();
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(123456));
        }

        #[test]
        fn put_varint_8_bytes() {
            let mut bytes = BytesMut::new();
            bytes.put_varint(15000000000).unwrap();
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(15000000000));
        }

        #[test]
        fn put_string_ok() {
            let mut bytes = BytesMut::new();
            bytes.put_string("abcd").unwrap();
            let mut bytes = std::io::Cursor::new(&bytes[..]);
            let result = bytes.try_get_string().unwrap();
            assert_eq!(result, "abcd");
        }
    }

    mod failure {
        use crate::modules::extensions::buf_put_ext::BufPutExt;
        use bytes::BytesMut;

        #[test]
        fn put_varint_of_2_62_fails_and_writes_nothing() {
            // Arrange: the smallest value only a draft-18 vi64 carries
            let mut bytes = BytesMut::new();

            // Act
            let result = bytes.put_varint(1 << 62);

            // Assert
            assert!(result.is_err());
            assert!(bytes.is_empty());
        }

        #[test]
        fn put_tuple_of_no_fields_fails_and_writes_nothing() {
            // Arrange
            let mut bytes = BytesMut::new();

            // Act
            let result = bytes.put_tuple(&[]);

            // Assert
            assert!(result.is_err());
            assert!(bytes.is_empty());
        }
    }

    mod properties {
        use bytes::{Buf, BytesMut};
        use proptest::{prelude::*, sample::Index};

        use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};
        use crate::modules::test_support::control_message_strategies::varint;

        proptest! {
            #[test]
            fn a_varint_reads_back_however_its_bytes_are_split(value in varint(), split in any::<Index>()) {
                // Arrange
                let mut bytes = BytesMut::new();
                bytes.put_varint(value).unwrap();
                let (head, tail) = bytes.split_at(split.index(bytes.len() + 1));
                let mut chunks = head.chain(tail);

                // Act
                let decoded = chunks.try_get_varint();

                // Assert
                prop_assert_eq!(decoded, Ok(value));
                prop_assert_eq!(chunks.remaining(), 0);
            }
        }
    }
}

use anyhow::bail;
use bytes::{Buf, BufMut, BytesMut, TryGetError};

pub(crate) trait BufGetExt: BufMut {
    fn try_get_varint(&mut self) -> Result<u64, TryGetError>;
    fn try_get_string(&mut self) -> anyhow::Result<String>;
}

impl BufGetExt for BytesMut {
    fn try_get_varint(&mut self) -> Result<u64, TryGetError> {
        if let Some(head) = self.first() {
            let def_head = *head;
            match def_head >> 6 {
                0 => match self.try_get_u8() {
                    Ok(v) => Ok(v as u64),
                    Err(e) => Err(e),
                },
                1 => match self.try_get_u16() {
                    Ok(v) => Ok((v & 0x3FFF) as u64),
                    Err(e) => Err(e),
                },
                2 => match self.try_get_u32() {
                    Ok(v) => Ok((v & 0x3FFF_FFFF) as u64),
                    Err(e) => Err(e),
                },
                3 => match self.try_get_u64() {
                    Ok(v) => Ok(v & 0x3FFF_FFFF_FFFF_FFFF),
                    Err(e) => Err(e),
                },
                _ => unreachable!(),
            }
        } else {
            Err(TryGetError {
                requested: 1,
                available: self.remaining(),
            })
        }
    }

    fn try_get_string(&mut self) -> anyhow::Result<String> {
        let length = match self.try_get_varint() {
            Ok(v) => v,
            Err(e) => {
                bail!("Failed to get string length: {:?}", e)
            }
        };
        let bytes_str = self.copy_to_bytes(length as usize);
        let s = str::from_utf8(&bytes_str)
            .inspect_err(|e| tracing::error!("Failed to convert bytes to string: {:?}", e))?;
        Ok(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::extensions::buf_get_ext::BufGetExt;
        use bytes::BytesMut;

        #[test]
        fn try_get_varint_1_byte() {
            let mut bytes = BytesMut::from(&[3, 4, 5, 6][..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(3));
        }

        #[test]
        fn try_get_varint_2_bytes() {
            let mut bytes = BytesMut::from(&[70, 5, 15, 27][..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(1541));
        }

        #[test]
        fn try_get_varint_4_bytes() {
            let mut bytes = BytesMut::from(&[128, 2, 0, 0, 8, 5][..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(131072));
        }

        #[test]
        fn try_get_varint_8_bytes() {
            let mut bytes = BytesMut::from(&[240, 0, 0, 0, 0, 0, 0, 14, 9][..]);
            let result = bytes.try_get_varint();
            assert_eq!(result, Ok(3458764513820540942));
        }

        #[test]
        fn try_get_string_ok() {
            let mut bytes = BytesMut::from(&[0x04, 0x61, 0x62, 0x63, 0x64][..]);
            let result = bytes.try_get_string();
            assert_eq!(result.unwrap(), "abcd".to_string());
        }
    }
    mod failure {
        use bytes::BytesMut;

        use crate::modules::extensions::buf_get_ext::BufGetExt;

        #[test]
        fn try_get_varint_failed() {
            let mut bytes = BytesMut::new();
            let result = bytes.try_get_varint();
            assert!(result.is_err());
        }

        #[test]
        fn try_get_varint_2_bytes_failed() {
            let mut bytes = BytesMut::from(&[64][..]);
            let result = bytes.try_get_varint();
            assert!(result.is_err());
        }

        #[test]
        fn try_get_varint_4_bytes_failed() {
            let mut bytes = BytesMut::from(&[128, 0][..]);
            let result = bytes.try_get_varint();
            assert!(result.is_err());
        }

        #[test]
        fn try_get_varint_8_bytes_failed() {
            let mut bytes = BytesMut::from(&[255, 0][..]);
            let result = bytes.try_get_varint();
            assert!(result.is_err());
        }

        #[test]
        fn try_get_string_failed() {
            let mut bytes = BytesMut::new();
            let result = bytes.try_get_string();
            assert!(result.is_err());
        }
    }
}

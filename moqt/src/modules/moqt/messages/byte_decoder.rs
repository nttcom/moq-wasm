use bytes::{Buf, BytesMut};

pub(crate) struct ByteDecoder;

impl ByteDecoder {
    pub(crate) fn get_u8(bytes: &mut BytesMut) -> u8 {
        bytes.get_u8()
    }

    pub(crate) fn get_u16(bytes: &mut BytesMut) -> u16 {
        let head = bytes.get_u16();
        head & 0x3FFF
    }

    pub(crate) fn get_u32(bytes: &mut BytesMut) -> u32 {
        let head = bytes.get_u32();
        head & 0x3FFF_FFFF
    }

    pub(crate) fn get_u64(bytes: &mut BytesMut) -> u64 {
        let head = bytes.get_u64();
        head & 0x3FFF_FFFF_FFFF_FFFF
    }

    pub(crate) fn get_varint(bytes: &mut BytesMut) -> Option<u64> {
        if let Some(head) = bytes.first() {
            let def_head = *head;
            let result = match def_head >> 6 {
                0 => Self::get_u8(bytes) as u64,
                1 => Self::get_u16(bytes) as u64,
                2 => Self::get_u32(bytes) as u64,
                3 => Self::get_u64(bytes),
                _ => unreachable!(),
            };
            Some(result)
        } else {
            None
        }
    }

    pub(crate) fn get_string(bytes: &mut BytesMut) -> Option<(u64, String)> {
        let length = Self::get_varint(bytes)?;
        let bytes_str = bytes.copy_to_bytes(length as usize);
        let s = str::from_utf8(&bytes_str)
            .inspect_err(|e| tracing::error!("Failed to convert bytes to string: {:?}", e))
            .ok()?;
        Some((length, s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn get_u8_ok() {
        let mut bytes = BytesMut::from(&[1, 2, 3, 4][..]);
        let result = ByteDecoder::get_u8(&mut bytes);
        assert_eq!(result, 1);
    }

    #[test]
    fn get_u16_within_u8_range_ok() {
        let mut bytes = BytesMut::from(&[64, 2][..]);
        let result = ByteDecoder::get_u16(&mut bytes);
        assert_eq!(result, 2);
    }

    #[test]
    fn get_u16_ok() {
        let mut bytes = BytesMut::from(&[64, 70][..]);
        let result = ByteDecoder::get_u16(&mut bytes);
        assert_eq!(result, 70);
    }

    #[test]
    fn get_u32_within_u16_range_ok() {
        let mut bytes = BytesMut::from(&[128, 0, 0, 85][..]);
        let result = ByteDecoder::get_u32(&mut bytes);
        assert_eq!(result, 85);
    }

    #[test]
    fn get_u32_ok() {
        let mut bytes = BytesMut::from(&[128, 1, 1, 2][..]);
        let result = ByteDecoder::get_u32(&mut bytes);
        assert_eq!(result, 65794);
    }

    #[test]
    fn get_varint_1_byte() {
        let mut bytes = BytesMut::from(&[3, 4, 5, 6][..]);
        let result = ByteDecoder::get_varint(&mut bytes);
        assert_eq!(result, Some(3));
    }

    #[test]
    fn get_varint_2_bytes() {
        let mut bytes = BytesMut::from(&[64, 5, 15, 27][..]);
        let result = ByteDecoder::get_varint(&mut bytes);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn get_varint_4_bytes() {
        let mut bytes = BytesMut::from(&[128, 0, 0, 8, 5][..]);
        let result = ByteDecoder::get_varint(&mut bytes);
        assert_eq!(result, Some(8));
    }

    #[test]
    fn get_varint_8_bytes() {
        let mut bytes = BytesMut::from(&[192, 0, 0, 0, 0, 0, 0, 14, 9][..]);
        let result = ByteDecoder::get_varint(&mut bytes);
        assert_eq!(result, Some(14));
    }

    #[test]
    fn get_string_ok() {
        let mut bytes = BytesMut::from(&[0x04, 0x61, 0x62, 0x63, 0x64][..]);
        let (len, result) = ByteDecoder::get_string(&mut bytes).unwrap();
        assert_eq!(len, 4);
        assert_eq!(result, "abcd");
    }
}

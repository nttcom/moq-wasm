use anyhow::{Result, bail};
use bytes::{Buf, BufMut, BytesMut};
use std::io::Cursor;

// See https://datatracker.ietf.org/doc/html/rfc9000#name-variable-length-integer-enc

pub fn read_variable_integer_from_buffer(buf: &mut BytesMut) -> Result<u64> {
    let mut cur = Cursor::new(&buf[..]);

    let ret = read_variable_integer(&mut cur);

    buf.advance(cur.position() as usize);

    ret
}

pub fn read_variable_integer(buf: &mut std::io::Cursor<&[u8]>) -> Result<u64> {
    if buf.remaining() == 0 {
        bail!("buffer is empty in read_variable_integer");
    }

    let first_byte = buf.get_u8();
    let mut value: u64 = (first_byte % 64).into();
    let rest_len = get_2msb_length_from_first_byte(first_byte) - 1;

    if buf.remaining() < rest_len.into() {
        bail!(
            "buffer does not have enough length. actual: {}, expected: {}",
            buf.remaining() + 1,
            rest_len + 1
        )
    }

    for _ in 0..rest_len {
        let next_byte = buf.get_u8();
        value = value * 256 + next_byte as u64;
    }

    Ok(value)
}

pub fn get_2msb_length_from_first_byte(first_byte: u8) -> u8 {
    let msb2 = first_byte / 64;

    // 2MSB    Length
    // 00  ->  1
    // 01  ->  2
    // 10  ->  4
    // 11  ->  8
    2usize.pow(msb2 as u32) as u8
}

pub fn get_2msb_value(value: u64) -> u64 {
    let first_byte = (value & 0xFF) as u8; // 0xFF: Bit mask to get the first byte
    let first_two_bits = first_byte / 64;
    match first_two_bits {
        0b00 => value & 0x3F,               // 8ビットの先頭2bitを無視
        0b01 => value & 0x3FFF,             // 16ビットの先頭2bitを無視
        0b10 => value & 0x3FFFFFFF,         // 32ビットの先頭2bitを無視
        0b11 => value & 0x3FFFFFFFFFFFFFFF, // 64ビットの先頭2bitを無視
        _ => unreachable!(),
    }
}

pub fn write_variable_integer(value: u64) -> BytesMut {
    let mut buf = BytesMut::with_capacity(8);

    if value < 0x40 {
        buf.put_u8(value as u8)
    } else if value < 0x4000 {
        buf.put_u16(value as u16 ^ 0x4000)
    } else if value < 0x40000000 {
        buf.put_u32(value as u32 ^ 0x80000000)
    } else if value < 0x4000000000000000 {
        buf.put_u64(value ^ 0xc000000000000000)
    } else {
        unreachable!("Invalid use of `write_variable_integer` with {}", value);
    }

    buf
}

#[cfg(test)]
mod decoder {
    use bytes::{Buf, BufMut, BytesMut};

    use crate::variable_integer::read_variable_integer;

    use std::io::Cursor;

    #[test]
    fn decode_single_byte_1() {
        let mut buf = BytesMut::with_capacity(0);
        buf.put_u8(0x05);
        buf.put_u32(0xdeadbeef);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf).unwrap();

        assert_eq!(decoded_value, 0x05);
        assert_eq!(buf.remaining(), 4);
    }
    #[test]
    fn decode_single_byte_2() {
        let mut buf = BytesMut::with_capacity(0);
        buf.put_u8(0x3f);
        buf.put_u32(0xdeadbeef);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf).unwrap();

        assert_eq!(decoded_value, 0x3f);
        assert_eq!(buf.remaining(), 4);
    }
    #[test]
    fn decode_two_bytes() {
        let mut buf = BytesMut::with_capacity(0);
        buf.put_u16(0x7fec);
        buf.put_u8(0x05);
        buf.put_u32(0xdeadbeef);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf).unwrap();

        assert_eq!(decoded_value, 0x3fec);
        assert_eq!(buf.remaining(), 5);
    }
    #[test]
    fn decode_four_bytes() {
        let mut buf = BytesMut::with_capacity(0);
        buf.put_u32(0xbaaaaaad);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf).unwrap();

        assert_eq!(decoded_value, 0x3aaaaaad);
        assert_eq!(buf.remaining(), 0);
    }
    #[test]
    fn decode_eight_bytes() {
        let mut buf = BytesMut::with_capacity(0);
        buf.put_u64(0xdeadbeefbaaaaaad);
        buf.put_u8(0x00);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf).unwrap();

        assert_eq!(decoded_value, 0x1eadbeefbaaaaaad);
        assert_eq!(buf.remaining(), 1);
    }

    #[test]
    fn decode_failed_by_first_byte() {
        let buf = BytesMut::with_capacity(0);

        let mut buf = Cursor::new(&buf[..]);
        let decoded_value = read_variable_integer(&mut buf);

        assert!(decoded_value.is_err());
    }
}

#[cfg(test)]
mod encoder {
    use bytes::Buf;

    use crate::variable_integer::write_variable_integer;

    #[test]
    fn encode_single_byte_1() {
        let value: u64 = 0x05;
        let mut buf = write_variable_integer(value);

        assert_eq!(buf.get_u8(), 0x05);
    }
    #[test]
    fn encode_single_byte_2() {
        let value: u64 = 0x3f;
        let mut buf = write_variable_integer(value);

        assert_eq!(buf.get_u8(), 0x3f);
    }
    #[test]
    fn encode_two_bytes() {
        let value: u64 = 0x3fec;
        let mut buf = write_variable_integer(value);

        assert_eq!(buf.get_u16(), 0x7fec);
    }
    #[test]
    fn encode_four_bytes() {
        let value: u64 = 0x3aaaaaad;
        let mut buf = write_variable_integer(value);

        assert_eq!(buf.get_u32(), 0xbaaaaaad);
    }
    #[test]
    fn encode_eight_bytes() {
        let value: u64 = 0x1eadbeefbaaaaaad;
        let mut buf = write_variable_integer(value);

        assert_eq!(buf.get_u64(), 0xdeadbeefbaaaaaad);
    }
}

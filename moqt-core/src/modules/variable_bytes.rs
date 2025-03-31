use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use std::io::Cursor;

use crate::variable_integer::{read_variable_integer, write_variable_integer};

// See https://datatracker.ietf.org/doc/html/draft-ietf-moq-transport-01#name-notational-conventions

pub fn read_bytes_from_buffer(buf: &mut BytesMut, length: usize) -> Result<Vec<u8>> {
    //! this function is used for x (A) format.
    //! x (A): Indicates that x is A bits long

    let mut cur = Cursor::new(&buf[..]);
    let ret = read_bytes(&mut cur, length);

    buf.advance(cur.position() as usize);

    ret
}

pub fn read_bytes(buf: &mut std::io::Cursor<&[u8]>, length: usize) -> Result<Vec<u8>> {
    if buf.remaining() == 0 {
        bail!("buffer is empty in read_variable_bytes");
    }

    if buf.remaining() < length {
        bail!(
            "buffer does not have enough length. actual: {}, expected: {}",
            buf.remaining() + 1,
            length + 1
        )
    }
    let value = buf.get_ref()[buf.position() as usize..buf.position() as usize + length]
        .as_ref()
        .to_vec();

    buf.advance(length);

    Ok(value)
}

pub fn read_variable_bytes_from_buffer(buf: &mut BytesMut) -> Result<Vec<u8>> {
    //!  this function is used for x (b) format.
    //!  x (b) : Indicates that x consists of a variable length integer, followed by that many bytes of binary data.

    let mut cur = Cursor::new(&buf[..]);
    let ret = read_variable_bytes(&mut cur);

    buf.advance(cur.position() as usize);

    ret
}

pub fn read_variable_bytes(buf: &mut std::io::Cursor<&[u8]>) -> Result<Vec<u8>> {
    if buf.remaining() == 0 {
        bail!("buffer is empty in read_variable_bytes");
    }

    let len = read_variable_integer(buf)? as usize;

    if buf.remaining() < len {
        bail!(
            "buffer does not have enough length. actual: {}, expected: {}",
            buf.remaining() + 1,
            len + 1
        )
    }
    let value = buf.get_ref()[buf.position() as usize..buf.position() as usize + len]
        .as_ref()
        .to_vec();

    buf.advance(value.len());

    Ok(value)
}

pub fn read_all_variable_bytes(buf: &mut std::io::Cursor<&[u8]>) -> Result<Vec<u8>> {
    if buf.remaining() == 0 {
        bail!("buffer is empty in read_variable_bytes");
    }

    let len = buf.remaining();

    let value = buf.get_ref()[buf.position() as usize..buf.position() as usize + len]
        .as_ref()
        .to_vec();

    buf.advance(value.len());

    Ok(value)
}

pub fn write_variable_bytes(value: &Vec<u8>) -> BytesMut {
    //!  this function is used for x (b) format.
    //!  x (b) : Indicates that x consists of a variable length integer, followed by that many bytes of binary data.
    let mut buf = BytesMut::with_capacity(0);
    let len = value.len();
    buf.extend(write_variable_integer(len as u64));
    buf.extend(value);

    buf
}

pub fn write_bytes(value: &Vec<u8>) -> BytesMut {
    //! this function is used for x (A) format.
    //! x (A): Indicates that x is A bits long
    let mut buf = BytesMut::with_capacity(0);
    buf.extend(value);

    buf
}

pub fn bytes_to_integer(value: Vec<u8>) -> Result<u64> {
    let mut ret = 0;
    for (i, &byte) in value.iter().enumerate() {
        ret |= (byte as u64) << (i * 8);
    }

    Ok(ret)
}

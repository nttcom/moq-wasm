use std::io::Cursor;

use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};

// See https://datatracker.ietf.org/doc/html/draft-ietf-moq-transport-01#name-notational-conventions

pub fn read_variable_bytes_from_buffer(buf: &mut BytesMut) -> Result<Vec<u8>> {
    let mut cur = Cursor::new(&buf[..]);
    let ret = read_variable_bytes(&mut cur);

    buf.advance(cur.position() as usize);

    ret
}

pub fn read_variable_bytes(buf: &mut std::io::Cursor<&[u8]>) -> Result<Vec<u8>> {
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

pub fn read_variable_bytes_to_end_from_buffer(buf: &mut BytesMut) -> Result<Vec<u8>> {
    let mut cur = Cursor::new(&buf[..]);

    let ret = read_variable_bytes_to_end(&mut cur);

    buf.advance(cur.position() as usize);

    ret
}

pub fn read_variable_bytes_to_end(buf: &mut std::io::Cursor<&[u8]>) -> Result<Vec<u8>> {
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
    let mut buf = BytesMut::with_capacity(0);

    buf.extend(value);

    buf
}

pub fn write_variable_bytes_to_end(value: &Vec<u8>) -> BytesMut {
    let mut buf = BytesMut::with_capacity(0);

    buf.extend(value);

    buf
}

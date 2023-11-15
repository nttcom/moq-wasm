use std::io::Cursor;

use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};

use super::variable_integer::{read_variable_integer, write_variable_integer};

pub(crate) fn read_variable_bytes_from_buffer(buf: &mut BytesMut) -> Result<Vec<u8>> {
    let mut cur = Cursor::new(&buf[..]);

    let ret = read_variable_bytes(&mut cur);

    buf.advance(cur.position() as usize);

    ret
}

pub(crate) fn read_variable_bytes(buf: &mut std::io::Cursor<&[u8]>) -> Result<Vec<u8>> {
    if buf.remaining() == 0 {
        bail!("buffer is empty");
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

pub(crate) fn write_variable_bytes(value: &Vec<u8>) -> BytesMut {
    let mut buf = BytesMut::with_capacity(0);

    let len = value.len();
    buf.extend(write_variable_integer(len as u64));
    buf.extend(value);

    buf
}

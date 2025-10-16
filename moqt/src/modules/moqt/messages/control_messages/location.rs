use std::io::Cursor;

use bytes::BytesMut;

use crate::modules::moqt::messages::{
    moqt_message::MOQTMessage,
    moqt_message_error::MOQTMessageError,
    variable_integer::{read_variable_integer, write_variable_integer},
};

pub(super) struct Location {
    pub(super) group_id: u64,
    pub(super) object_id: u64,
}

impl MOQTMessage for Location {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self, MOQTMessageError> {
        let mut read_cur = Cursor::new(&buf[..]);
        let group_id = read_variable_integer(&mut read_cur)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        let object_id = read_variable_integer(&mut read_cur)
            .map_err(|_| MOQTMessageError::ProtocolViolation)?;
        Ok(Self {
            group_id,
            object_id,
        })
    }

    fn packetize(&self) -> bytes::BytesMut {
        let mut payload = BytesMut::new();
        payload.extend(write_variable_integer(self.group_id as u64));
        payload.extend(write_variable_integer(self.object_id as u64));
        payload
    }
}

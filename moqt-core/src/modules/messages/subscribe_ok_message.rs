use crate::modules::{
    variable_bytes::{read_variable_bytes_from_buffer, write_variable_bytes},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

pub(crate) struct SubscribeOk {
    track_namespace: String,
    track_name: String,
    track_id: u64,
    expires: u64,
}

impl SubscribeOk {
    pub fn new(
        track_namespace: String,
        track_name: String,
        track_id: u64,
        expires: u64,
    ) -> SubscribeOk {
        SubscribeOk {
            track_namespace,
            track_name,
            track_id,
            expires,
        }
    }
}

impl MOQTPayload for SubscribeOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let full_track_namespace = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let full_track_name = String::from_utf8(read_variable_bytes_from_buffer(buf)?)?;
        let track_id = read_variable_integer_from_buffer(buf)?;
        let expires = read_variable_integer_from_buffer(buf)?;

        Ok(SubscribeOk {
            track_namespace: full_track_namespace,
            track_name: full_track_name,
            track_id,
            expires,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
        buf.extend(write_variable_bytes(&self.track_name.as_bytes().to_vec()));
        buf.extend(write_variable_integer(self.track_id));
        buf.extend(write_variable_integer(self.expires));
    }
}

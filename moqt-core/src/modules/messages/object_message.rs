use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    variable_bytes::{
        read_variable_bytes_from_buffer, read_variable_bytes_to_end_from_buffer,
        write_variable_bytes,
    },
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, Serialize)]
pub struct ObjectMessageWithPayloadLength {
    track_id: u64,
    group_sequence: u64,
    object_sequence: u64,
    object_send_order: u64,
    object_payload_length: u64,
    object_payload: Vec<u8>,
}

impl ObjectMessageWithPayloadLength {
    pub fn new(
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Self {
        let object_payload_length = object_payload.len() as u64;

        ObjectMessageWithPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload_length,
            object_payload,
        }
    }
}

impl MOQTPayload for ObjectMessageWithPayloadLength {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let group_sequence = read_variable_integer_from_buffer(buf).context("group sequence")?;
        let object_sequence = read_variable_integer_from_buffer(buf).context("object sequence")?;
        let object_send_order =
            read_variable_integer_from_buffer(buf).context("object send order")?;
        let object_payload = read_variable_bytes_from_buffer(buf).context("object payload")?;
        let object_payload_length = object_payload.len() as u64;

        Ok(ObjectMessageWithPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload_length,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.track_id));
        buf.extend(write_variable_integer(self.group_sequence));
        buf.extend(write_variable_integer(self.object_sequence));
        buf.extend(write_variable_integer(self.object_send_order));
        buf.extend(write_variable_bytes(&self.object_payload));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectMessageWithoutPayloadLength {
    track_id: u64,
    group_sequence: u64,
    object_sequence: u64,
    object_send_order: u64,
    object_payload: Vec<u8>,
}

impl ObjectMessageWithoutPayloadLength {
    pub fn new(
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Self {
        ObjectMessageWithoutPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload,
        }
    }
}

impl MOQTPayload for ObjectMessageWithoutPayloadLength {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let group_sequence = read_variable_integer_from_buffer(buf).context("group sequence")?;
        let object_sequence = read_variable_integer_from_buffer(buf).context("object sequence")?;
        let object_send_order =
            read_variable_integer_from_buffer(buf).context("object send order")?;
        let object_payload =
            read_variable_bytes_to_end_from_buffer(buf).context("object payload")?;

        Ok(ObjectMessageWithoutPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.track_id));
        buf.extend(write_variable_integer(self.group_sequence));
        buf.extend(write_variable_integer(self.object_sequence));
        buf.extend(write_variable_integer(self.object_send_order));
        buf.extend(write_variable_bytes(&self.object_payload));
    }
}

use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::{
    variable_bytes::{
        read_fixed_length_bytes_from_buffer, read_variable_bytes_to_end_from_buffer,
        write_variable_bytes,
    },
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ObjectWithPayloadLength {
    track_id: u64,
    group_sequence: u64,
    object_sequence: u64,
    object_send_order: u64,
    object_payload_length: u64,
    object_payload: Vec<u8>,
}

impl ObjectWithPayloadLength {
    pub fn new(
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Self {
        let object_payload_length = object_payload.len() as u64;

        ObjectWithPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload_length,
            object_payload,
        }
    }

    pub fn track_id(&self) -> u64 {
        self.track_id
    }
}

impl MOQTPayload for ObjectWithPayloadLength {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let group_sequence = read_variable_integer_from_buffer(buf).context("group sequence")?;
        let object_sequence = read_variable_integer_from_buffer(buf).context("object sequence")?;
        let object_send_order =
            read_variable_integer_from_buffer(buf).context("object send order")?;
        let object_payload_length =
            read_variable_integer_from_buffer(buf).context("object payload length")?;

        // Skip the varint part that indicates the length of the payload
        let _ = read_variable_integer_from_buffer(buf)?;
        let object_payload =
            read_fixed_length_bytes_from_buffer(buf, object_payload_length as usize)
                .context("object payload")?;

        tracing::trace!("Depacketized Object With Payload Length message.");

        Ok(ObjectWithPayloadLength {
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
        buf.extend(write_variable_integer(self.object_payload_length));
        buf.extend(write_variable_bytes(&self.object_payload));

        tracing::trace!("Packetized Object With Payload Length message.");
    }
    /// Method to enable downcasting from MOQTPayload to ObjectWithPayloadLength
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ObjectWithoutPayloadLength {
    track_id: u64,
    group_sequence: u64,
    object_sequence: u64,
    object_send_order: u64,
    object_payload: Vec<u8>,
}

impl ObjectWithoutPayloadLength {
    pub fn new(
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Self {
        ObjectWithoutPayloadLength {
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload,
        }
    }

    pub fn track_id(&self) -> u64 {
        self.track_id
    }
}

impl MOQTPayload for ObjectWithoutPayloadLength {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let track_id = read_variable_integer_from_buffer(buf).context("track id")?;
        let group_sequence = read_variable_integer_from_buffer(buf).context("group sequence")?;
        let object_sequence = read_variable_integer_from_buffer(buf).context("object sequence")?;
        let object_send_order =
            read_variable_integer_from_buffer(buf).context("object send order")?;

        // Skip the varint part that indicates the length of the payload
        let _ = read_variable_integer_from_buffer(buf)?;
        let object_payload =
            read_variable_bytes_to_end_from_buffer(buf).context("object payload")?;

        tracing::trace!("Depacketized Object Without Payload Length message.");

        Ok(ObjectWithoutPayloadLength {
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

        tracing::trace!("Packetized Object Without Payload Length message.");
    }
    /// Method to enable downcasting from MOQTPayload to ObjectWithoutPayloadLength
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::moqt_payload::MOQTPayload;
    use crate::messages::object::{ObjectWithPayloadLength, ObjectWithoutPayloadLength};
    use bytes::BytesMut;
    #[test]
    fn packetize_object_with_payload_length() {
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_with_payload_length = ObjectWithPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        let mut buf = bytes::BytesMut::new();
        object_with_payload_length.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload Length (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_object_with_payload_length() {
        let bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload Length (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_with_payload_length =
            ObjectWithPayloadLength::depacketize(&mut buf).unwrap();

        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let expected_object_with_payload_length = ObjectWithPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        assert_eq!(
            depacketized_object_with_payload_length,
            expected_object_with_payload_length
        );
    }

    #[test]
    fn packetize_object_without_payload_length() {
        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let object_without_payload_length = ObjectWithoutPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        let mut buf = bytes::BytesMut::new();
        object_without_payload_length.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_object_without_payload_length() {
        let bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_with_payload_length =
            ObjectWithoutPayloadLength::depacketize(&mut buf).unwrap();

        let track_id = 0;
        let group_sequence = 1;
        let object_sequence = 2;
        let object_send_order = 3;
        let object_payload = vec![0, 1, 2];

        let expected_object_with_payload_length = ObjectWithoutPayloadLength::new(
            track_id,
            group_sequence,
            object_sequence,
            object_send_order,
            object_payload.clone(),
        );

        assert_eq!(
            depacketized_object_with_payload_length,
            expected_object_with_payload_length
        );
    }
}

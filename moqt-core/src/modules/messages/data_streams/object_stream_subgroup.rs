use crate::messages::data_streams::object_status::ObjectStatus;
use crate::{
    variable_bytes::read_fixed_length_bytes_from_buffer,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::messages::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ObjectStreamSubgroup {
    object_id: u64,
    object_payload_length: u64,
    object_status: Option<ObjectStatus>,
    object_payload: Vec<u8>,
}

impl ObjectStreamSubgroup {
    pub fn new(
        object_id: u64,
        object_status: Option<ObjectStatus>,
        object_payload: Vec<u8>,
    ) -> Result<Self> {
        let object_payload_length = object_payload.len() as u64;

        // Any object with a status code other than zero MUST have an empty payload.
        if let Some(status) = object_status {
            if status != ObjectStatus::Normal && object_payload_length != 0 {
                // TODO: return Termination Error Code
                bail!("Any object with a status code other than zero MUST have an empty payload.");
            }
        }

        Ok(ObjectStreamSubgroup {
            object_id,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }
}

impl MOQTPayload for ObjectStreamSubgroup {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let object_id = read_variable_integer_from_buffer(buf).context("object id")?;
        let object_payload_length =
            read_variable_integer_from_buffer(buf).context("object payload length")?;

        // If the length of the remaining buf is larger than object_payload_length, object_status exists.
        let object_status = if buf.len() > object_payload_length as usize {
            let object_status_u64 = read_variable_integer_from_buffer(buf)?;
            let object_status =
                match ObjectStatus::try_from(object_status_u64 as u8).context("object status") {
                    Ok(status) => status,
                    Err(err) => {
                        // Any other value SHOULD be treated as a Protocol Violation and terminate the session with a Protocol Violation
                        // TODO: return Termination Error Code
                        bail!(err);
                    }
                };

            // Any object with a status code other than zero MUST have an empty payload.
            if object_status != ObjectStatus::Normal && object_payload_length != 0 {
                // TODO: return Termination Error Code
                bail!("Any object with a status code other than zero MUST have an empty payload.");
            }

            Some(object_status)
        } else {
            None
        };

        let object_payload = if object_payload_length > 0 {
            read_fixed_length_bytes_from_buffer(buf, object_payload_length as usize)
                .context("object payload")?
        } else {
            vec![]
        };

        tracing::trace!("Depacketized Object Stream Subgroup message.");

        Ok(ObjectStreamSubgroup {
            object_id,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.object_id));
        buf.extend(write_variable_integer(self.object_payload_length));
        if self.object_status.is_some() {
            buf.extend(write_variable_integer(
                u8::from(self.object_status.unwrap()) as u64,
            ));
        }
        buf.extend(&self.object_payload);

        tracing::trace!("Packetized Object Stream Subgroup message.");
    }
    /// Method to enable downcasting from MOQTPayload to ObjectStreamSubgroup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use crate::messages::data_streams::{
        object_status::ObjectStatus, object_stream_subgroup::ObjectStreamSubgroup,
    };
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;

    #[test]
    fn packetize_object_stream_subgroup_normal() {
        let object_id = 0;
        let object_status = Some(ObjectStatus::Normal);
        let object_payload = vec![0, 1, 2];

        let object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        let mut buf = bytes::BytesMut::new();
        object_stream_subgroup.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Object ID (i)
            3, // Object Payload Length (i)
            0, // Object Status (i)
            0, 1, 2, // Object Payload (..)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn packetize_object_stream_subgroup_normal_and_empty_payload() {
        let object_id = 0;
        let object_status = Some(ObjectStatus::Normal);
        let object_payload = vec![];

        let object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        let mut buf = bytes::BytesMut::new();
        object_stream_subgroup.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Object ID (i)
            0, // Object Payload Length (i)
            0, // Object Status (i)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn packetize_object_stream_subgroup_not_normal() {
        let object_id = 0;
        let object_status = Some(ObjectStatus::EndOfGroup);
        let object_payload = vec![];

        let object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        let mut buf = bytes::BytesMut::new();
        object_stream_subgroup.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Object ID (i)
            0, // Object Payload Length (i)
            3, // Object Status (i)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_object_stream_subgroup_normal() {
        let bytes_array = [
            0, // Object ID (i)
            3, // Object Payload Length (i)
            0, // Object Status (i)
            0, 1, 2, // Object Payload (..)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_stream_subgroup =
            ObjectStreamSubgroup::depacketize(&mut buf).unwrap();

        let object_id = 0;
        let object_status = Some(ObjectStatus::Normal);
        let object_payload = vec![0, 1, 2];

        let expected_object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        assert_eq!(
            depacketized_object_stream_subgroup,
            expected_object_stream_subgroup
        );
    }

    #[test]
    fn depacketize_object_stream_subgroup_normal_and_empty_payload() {
        let bytes_array = [
            0, // Object ID (i)
            0, // Object Payload Length (i)
            0, // Object Status (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_stream_subgroup =
            ObjectStreamSubgroup::depacketize(&mut buf).unwrap();

        let object_id = 0;
        let object_status = Some(ObjectStatus::Normal);
        let object_payload = vec![];

        let expected_object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        assert_eq!(
            depacketized_object_stream_subgroup,
            expected_object_stream_subgroup
        );
    }

    #[test]
    fn depacketize_object_stream_subgroup_not_normal() {
        let bytes_array = [
            0, // Object ID (i)
            0, // Object Payload Length (i)
            1, // Object Status (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_stream_subgroup =
            ObjectStreamSubgroup::depacketize(&mut buf).unwrap();

        let object_id = 0;
        let object_status = Some(ObjectStatus::DoesNotExist);
        let object_payload = vec![];

        let expected_object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

        assert_eq!(
            depacketized_object_stream_subgroup,
            expected_object_stream_subgroup
        );
    }
}

#[cfg(test)]
mod failure {
    use crate::messages::data_streams::object_stream_subgroup::{
        ObjectStatus, ObjectStreamSubgroup,
    };
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;

    #[test]
    fn packetize_object_stream_subgroup_not_normal_and_not_empty_payload() {
        let object_id = 0;
        let object_status = Some(ObjectStatus::EndOfTrackAndGroup);
        let object_payload = vec![0, 1, 2];

        let object_stream_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, object_payload);

        assert!(object_stream_subgroup.is_err());
    }

    #[test]
    fn depacketize_object_stream_subgroup_not_normal_and_not_empty_payload() {
        let bytes_array = [
            0, // Object ID (i)
            3, // Object Payload Length (i)
            5, // Object Status (i)
            0, 1, 2, // Object Payload (..)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_stream_subgroup = ObjectStreamSubgroup::depacketize(&mut buf);

        assert!(depacketized_object_stream_subgroup.is_err());
    }

    #[test]
    #[should_panic]
    fn depacketize_object_stream_subgroup_wrong_parameter_length() {
        let bytes_array = [
            0, // Object ID (i)
            8, // Object Payload Length (i)
            0, // Object Status (i)
            0, 1, 2, // Object Payload (..)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);

        let _ = ObjectStreamSubgroup::depacketize(&mut buf);
    }

    #[test]
    fn depacketize_object_stream_subgroup_wrong_object_status() {
        let bytes_array = [
            0, // Object ID (i)
            0, // Object Payload Length (i)
            2, // Object Status (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let depacketized_object_stream_subgroup = ObjectStreamSubgroup::depacketize(&mut buf);

        assert!(depacketized_object_stream_subgroup.is_err());
    }
}

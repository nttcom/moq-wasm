use crate::{
    messages::data_streams::{object_status::ObjectStatus, DataStreams},
    variable_bytes::read_fixed_length_bytes,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{bail, Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ObjectDatagram {
    subscribe_id: u64,
    track_alias: u64,
    group_id: u64,
    object_id: u64,
    publisher_priority: u8,
    object_payload_length: u64,
    object_status: Option<ObjectStatus>,
    object_payload: Vec<u8>,
}

impl ObjectDatagram {
    pub fn new(
        subscribe_id: u64,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_status: Option<ObjectStatus>,
        object_payload: Vec<u8>,
    ) -> Result<Self> {
        let object_payload_length = object_payload.len() as u64;

        if object_status.is_some() && object_payload_length != 0 {
            bail!("The Object Status field is only sent if the Object Payload Length is zero.");
        }

        // Any object with a status code other than zero MUST have an empty payload.
        if let Some(status) = object_status {
            if status != ObjectStatus::Normal && object_payload_length != 0 {
                // TODO: return Termination Error Code
                bail!("Any object with a status code other than zero MUST have an empty payload.");
            }
        }

        Ok(ObjectDatagram {
            subscribe_id,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }

    pub fn object_status(&self) -> Option<ObjectStatus> {
        self.object_status
    }

    pub fn object_payload(&self) -> Vec<u8> {
        self.object_payload.clone()
    }
}

impl DataStreams for ObjectDatagram {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer(read_cur).context("subscribe id")?;
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let group_id = read_variable_integer(read_cur).context("group id")?;
        let object_id = read_variable_integer(read_cur).context("object id")?;
        let publisher_priority =
            read_fixed_length_bytes(read_cur, 1).context("publisher priority")?[0];
        let object_payload_length =
            read_variable_integer(read_cur).context("object payload length")?;

        // If the length of the remaining buf is larger than object_payload_length, object_status exists.
        // The Object Status field is only sent if the Object Payload Length is zero.
        let object_status = if object_payload_length == 0 {
            let object_status_u64 = read_variable_integer(read_cur)?;
            let object_status =
                match ObjectStatus::try_from(object_status_u64 as u8).context("object status") {
                    Ok(status) => status,
                    Err(err) => {
                        // Any other value SHOULD be treated as a Protocol Violation and terminate the session with a Protocol Violation
                        // TODO: return Termination Error Code
                        bail!(err);
                    }
                };

            Some(object_status)
        } else {
            None
        };

        let object_payload = if object_payload_length > 0 {
            read_fixed_length_bytes(read_cur, object_payload_length as usize)
                .context("object payload")?
        } else {
            vec![]
        };

        tracing::trace!("Depacketized Object Datagram message.");

        Ok(ObjectDatagram {
            subscribe_id,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        buf.extend(write_variable_integer(self.object_id));
        buf.extend(self.publisher_priority.to_be_bytes());
        buf.extend(write_variable_integer(self.object_payload_length));
        if self.object_status.is_some() {
            buf.extend(write_variable_integer(
                u8::from(self.object_status.unwrap()) as u64,
            ));
        }
        buf.extend(&self.object_payload);

        tracing::trace!("Packetized Object Datagram message.");
    }
    /// Method to enable downcasting from MOQTPayload to ObjectDatagram
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::messages::data_streams::{
            object_datagram::ObjectDatagram, object_status::ObjectStatus, DataStreams,
        };
        use bytes::BytesMut;
        use std::io::Cursor;

        #[test]
        fn packetize_object_datagram_normal() {
            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = None;
            let object_payload = vec![0, 1, 2];

            let object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            object_datagram.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_object_datagram_normal_and_empty_payload() {
            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            object_datagram.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_object_datagram_not_normal() {
            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = Some(ObjectStatus::EndOfGroup);
            let object_payload = vec![];

            let object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            object_datagram.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Object Payload Length (i)
                3, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_object_datagram_normal() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_object_datagram = ObjectDatagram::depacketize(&mut read_cur).unwrap();

            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = None;
            let object_payload = vec![0, 1, 2];

            let expected_object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_object_datagram, expected_object_datagram);
        }

        #[test]
        fn depacketize_object_datagram_normal_and_empty_payload() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_object_datagram = ObjectDatagram::depacketize(&mut read_cur).unwrap();

            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_object_datagram, expected_object_datagram);
        }

        #[test]
        fn depacketize_object_datagram_not_normal() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Object Payload Length (i)
                1, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_object_datagram = ObjectDatagram::depacketize(&mut read_cur).unwrap();

            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = Some(ObjectStatus::DoesNotExist);
            let object_payload = vec![];

            let expected_object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_object_datagram, expected_object_datagram);
        }
    }

    mod failure {
        use bytes::BytesMut;
        use std::io::Cursor;

        use crate::messages::data_streams::object_datagram::{
            DataStreams, ObjectDatagram, ObjectStatus,
        };

        #[test]
        fn packetize_object_datagram_not_normal_and_not_empty_payload() {
            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let object_status = Some(ObjectStatus::EndOfTrackAndGroup);
            let object_payload = vec![0, 1, 2];

            let object_datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            );

            assert!(object_datagram.is_err());
        }

        #[test]
        fn depacketize_object_datagram_wrong_object_status() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Object Payload Length (i)
                2, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_object_datagram = ObjectDatagram::depacketize(&mut read_cur);

            assert!(depacketized_object_datagram.is_err());
        }
    }
}

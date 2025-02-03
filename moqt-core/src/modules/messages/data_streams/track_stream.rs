use crate::{
    messages::data_streams::DataStreams,
    variable_bytes::read_fixed_length_bytes,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{bail, Context, Result};
use bytes::BytesMut;
use serde::Serialize;

use super::object_status::ObjectStatus;

/// Implementation of header message on QUIC Stream per Track.
/// TrackObject messages are sent following this message.
/// Type of Data Streams: STREAM_HEADER_TRACK (0x2)
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Header {
    subscribe_id: u64,
    track_alias: u64,
    publisher_priority: u8,
}

impl Header {
    pub fn new(subscribe_id: u64, track_alias: u64, publisher_priority: u8) -> Result<Self> {
        Ok(Header {
            subscribe_id,
            track_alias,
            publisher_priority,
        })
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }
}

impl DataStreams for Header {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer(read_cur).context("subscribe id")?;
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let publisher_priority =
            read_fixed_length_bytes(read_cur, 1).context("publisher priority")?[0];

        tracing::trace!("Depacketized Track Stream Header message.");

        Ok(Header {
            subscribe_id,
            track_alias,
            publisher_priority,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(self.publisher_priority.to_be_bytes());

        tracing::trace!("Packetized Track Stream Header message.");
    }
}

/// Implementation of object message on QUIC Stream per Track.
/// This message is sent following TrackHeader message.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Object {
    group_id: u64,
    object_id: u64,
    object_payload_length: u64,
    object_status: Option<ObjectStatus>,
    object_payload: Vec<u8>,
}

impl Object {
    pub fn new(
        group_id: u64,
        object_id: u64,
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

        Ok(Object {
            group_id,
            object_id,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    pub fn object_status(&self) -> Option<ObjectStatus> {
        self.object_status
    }
}

impl DataStreams for Object {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let group_id = read_variable_integer(read_cur).context("group id")?;
        let object_id = read_variable_integer(read_cur).context("object id")?;
        let object_payload_length =
            read_variable_integer(read_cur).context("object payload length")?;

        // If the length of the remaining buf is larger than object_payload_length, object_status exists.
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

        tracing::trace!("Depacketized Track Stream Object message.");

        Ok(Object {
            group_id,
            object_id,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.group_id));
        buf.extend(write_variable_integer(self.object_id));
        buf.extend(write_variable_integer(self.object_payload_length));
        if self.object_status.is_some() {
            buf.extend(write_variable_integer(
                u8::from(self.object_status.unwrap()) as u64,
            ));
        }
        buf.extend(&self.object_payload);

        tracing::trace!("Packetized Track Stream Object message.");
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::messages::data_streams::{
            object_status::ObjectStatus, track_stream, track_stream::DataStreams,
        };
        use bytes::BytesMut;
        use std::io::Cursor;

        #[test]
        fn packetize_track_stream_header() {
            let subscribe_id = 0;
            let track_alias = 1;
            let publisher_priority = 2;

            let track_stream_header =
                track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

            let mut buf = BytesMut::new();
            track_stream_header.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Subscriber Priority (8)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_track_stream_header() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Subscriber Priority (8)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_track_stream_header =
                track_stream::Header::depacketize(&mut read_cur).unwrap();

            let subscribe_id = 0;
            let track_alias = 1;
            let publisher_priority = 2;

            let expected_track_stream_header =
                track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

            assert_eq!(
                depacketized_track_stream_header,
                expected_track_stream_header
            );
        }

        #[test]
        fn packetize_track_stream_object_normal() {
            let group_id = 0;
            let object_id = 1;
            let object_status = None;
            let object_payload = vec![0, 1, 2];

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let mut buf = BytesMut::new();
            track_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_track_stream_object_normal_and_empty_payload() {
            let group_id = 0;
            let object_id = 1;
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let mut buf = BytesMut::new();
            track_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_track_stream_object_not_normal() {
            let group_id = 0;
            let object_id = 1;
            let object_status = Some(ObjectStatus::EndOfGroup);
            let object_payload = vec![];

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let mut buf = BytesMut::new();
            track_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                0, // Object Payload Length (i)
                3, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_track_stream_object_normal() {
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_track_stream_object =
                track_stream::Object::depacketize(&mut read_cur).unwrap();

            let group_id = 0;
            let object_id = 1;
            let object_status = None;
            let object_payload = vec![0, 1, 2];

            let expected_track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            assert_eq!(
                depacketized_track_stream_object,
                expected_track_stream_object
            );
        }

        #[test]
        fn depacketize_track_stream_object_normal_and_empty_payload() {
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_track_stream_object =
                track_stream::Object::depacketize(&mut read_cur).unwrap();

            let group_id = 0;
            let object_id = 1;
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            assert_eq!(
                depacketized_track_stream_object,
                expected_track_stream_object
            );
        }

        #[test]
        fn depacketize_track_stream_object_not_normal() {
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                0, // Object Payload Length (i)
                1, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_track_stream_object =
                track_stream::Object::depacketize(&mut read_cur).unwrap();

            let group_id = 0;
            let object_id = 1;
            let object_status = Some(ObjectStatus::DoesNotExist);
            let object_payload = vec![];

            let expected_track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            assert_eq!(
                depacketized_track_stream_object,
                expected_track_stream_object
            );
        }
    }

    mod failure {
        use bytes::BytesMut;
        use std::io::Cursor;

        use crate::messages::data_streams::{
            object_status::ObjectStatus, track_stream, DataStreams,
        };
        #[test]
        fn packetize_track_stream_object_not_normal_and_not_empty_payload() {
            let group_id = 0;
            let object_id = 1;
            let object_status = Some(ObjectStatus::EndOfTrackAndGroup);
            let object_payload = vec![0, 1, 2];

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload);

            assert!(track_stream_object.is_err());
        }

        #[test]
        fn depacketize_track_stream_object_wrong_object_status() {
            let bytes_array = [
                0, // Group ID (i)
                1, // Object ID (i)
                0, // Object Payload Length (i)
                2, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_track_stream_object = track_stream::Object::depacketize(&mut read_cur);

            assert!(depacketized_track_stream_object.is_err());
        }
    }
}

use super::{extension_header::ExtensionHeader, object_status::ObjectStatus};
use crate::{
    modules::session_handlers::messages::data_streams::DataStreams,
    modules::session_handlers::messages::variable_bytes::read_bytes,
    modules::session_handlers::messages::variable_integer::{
        read_variable_integer, write_variable_integer,
    },
};
use anyhow::{Context, Result, bail};
use bytes::{Buf, BytesMut};
use serde::Serialize;

/// Implementation of header message on QUIC Stream per Subgroup.
/// Object messages are sent following this message.
/// Type of Data Streams:SUBGROUP_HEADER (0x4)
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Header {
    track_alias: u64,
    group_id: u64,
    subgroup_id: u64,
    publisher_priority: u8,
}

impl Header {
    pub fn new(
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<Self> {
        Ok(Header {
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
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
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let group_id = read_variable_integer(read_cur).context("group id")?;
        let subgroup_id = read_variable_integer(read_cur).context("subgroup id")?;
        let publisher_priority = read_bytes(read_cur, 1).context("publisher priority")?[0];

        tracing::trace!("Depacketized Subgroup Stream Header message.");

        Ok(Header {
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        buf.extend(write_variable_integer(self.subgroup_id));
        buf.extend(self.publisher_priority.to_be_bytes());

        tracing::trace!("Packetized Subgroup Stream Header message.");
    }
}

/// Implementation of object message on QUIC Stream per Subgroup.
/// This message is sent following Header message.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Object {
    object_id: u64,
    extension_headers_length: u64,
    extension_headers: Vec<ExtensionHeader>,
    object_payload_length: u64,
    object_status: Option<ObjectStatus>,
    object_payload: Vec<u8>,
}

impl Object {
    pub fn new(
        object_id: u64,
        extension_headers: Vec<ExtensionHeader>,
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

        // length of total byte of extension headers
        let mut extension_headers_length = 0;
        for header in &extension_headers {
            extension_headers_length += header.byte_length() as u64;
        }

        Ok(Object {
            object_id,
            extension_headers_length,
            extension_headers,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    pub fn object_status(&self) -> Option<ObjectStatus> {
        self.object_status
    }

    pub fn object_payload_length(&self) -> u64 {
        self.object_payload_length
    }

    pub fn extension_headers_length(&self) -> u64 {
        self.extension_headers_length
    }

    pub fn extension_headers(&self) -> &Vec<ExtensionHeader> {
        &self.extension_headers
    }
}

impl DataStreams for Object {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let object_id = read_variable_integer(read_cur).context("object id")?;
        let extension_headers_length =
            read_variable_integer(read_cur).context("extension headers length")?;

        let mut extension_headers_vec = vec![];
        let extension_headers =
            read_bytes(read_cur, extension_headers_length as usize).context("extension headers")?;
        let mut extension_headers_cur = std::io::Cursor::new(&extension_headers[..]);

        while extension_headers_cur.has_remaining() {
            let extension_header = ExtensionHeader::depacketize(&mut extension_headers_cur)
                .context("extension header")?;
            extension_headers_vec.push(extension_header);
        }

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
            read_bytes(read_cur, object_payload_length as usize).context("object payload")?
        } else {
            vec![]
        };

        tracing::trace!("Depacketized Subgroup Stream Object message.");

        Ok(Object {
            object_id,
            extension_headers_length,
            extension_headers: extension_headers_vec,
            object_payload_length,
            object_status,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.object_id));

        buf.extend(write_variable_integer(self.extension_headers_length));
        for header in &self.extension_headers {
            header.packetize(buf);
        }

        buf.extend(write_variable_integer(self.object_payload_length));
        if let Some(status) = self.object_status {
            buf.extend(write_variable_integer(u8::from(status) as u64));
        }
        buf.extend(&self.object_payload);

        tracing::trace!("Packetized Subgroup Stream Object message.");
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::modules::session_handlers::messages::data_streams::extension_header::{
            ExtensionHeader, ExtensionHeaderValue, Value, ValueWithLength,
        };
        use crate::modules::session_handlers::messages::data_streams::{
            DataStreams, object_status::ObjectStatus, subgroup_stream,
        };
        use bytes::BytesMut;
        use std::io::Cursor;

        #[test]
        fn packetize_subgroup_stream_header() {
            let track_alias = 1;
            let group_id = 2;
            let subgroup_id = 3;
            let publisher_priority = 4;

            let subgroup_stream_header = subgroup_stream::Header::new(
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_header.packetize(&mut buf);

            let expected_bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Subgroup ID (i)
                4, // Subscriber Priority (8)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_subgroup_stream_header() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Subgroup ID (i)
                4, // Subscriber Priority (8)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_header =
                subgroup_stream::Header::depacketize(&mut read_cur).unwrap();

            let track_alias = 1;
            let group_id = 2;
            let subgroup_id = 3;
            let publisher_priority = 4;

            let expected_subgroup_stream_header = subgroup_stream::Header::new(
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_header,
                expected_subgroup_stream_header
            );
        }

        #[test]
        fn packetize_subgroup_stream_object_normal() {
            let object_id = 0;
            let extension_headers = vec![];
            let object_status = None;
            let object_payload = vec![0, 1, 2];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                3, // Object Payload Length (i)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_subgroup_stream_object_normal_and_empty_payload() {
            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_subgroup_stream_object_not_normal() {
            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::EndOfGroup);
            let object_payload = vec![];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                3, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_subgroup_stream_object_normal() {
            let bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_object,
                expected_subgroup_stream_object
            );
        }

        #[test]
        fn depacketize_subgroup_stream_object_normal_and_empty_payload() {
            let bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_object,
                expected_subgroup_stream_object
            );
        }

        #[test]
        fn depacketize_subgroup_stream_object_not_normal() {
            let bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                1, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::DoesNotExist);
            let object_payload = vec![];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_object,
                expected_subgroup_stream_object
            );
        }

        #[test]
        fn packetize_subgroup_stream_object_with_even_type_extension_header() {
            let object_id = 0;
            let header_type = 0;
            let value = 1;
            let header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_status = None;
            let object_payload = vec![1, 2, 3];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Object ID (i)
                2, // Extension Headers Length (i)
                0, // Header Type (i)
                1, // Header Value (i)
                3, // Object Payload Length (i)
                1, 2, 3, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_subgroup_stream_object_with_odd_type_extension_header() {
            let object_id = 0;
            let header_type = 1;
            let value = vec![116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53, 54];
            let header_value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0,  // Object ID (i)
                16, // Extension Headers Length (i)
                1,  // Header Type (i)
                14, // Header Length (i)
                116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53,
                54, // Header Value (..)
                0,  // Object Payload Length (i)
                0,  // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_subgroup_stream_object_with_mixed_type_extension_headers() {
            let object_id = 0;

            let even_header_type = 4;
            let even_value = 3;
            let even_header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(even_value));

            let odd_header_type = 5;
            let odd_value = vec![116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53, 54];
            let odd_header_value =
                ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(odd_value));

            let extension_headers = vec![
                ExtensionHeader::new(even_header_type, even_header_value).unwrap(),
                ExtensionHeader::new(odd_header_type, odd_header_value).unwrap(),
            ];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut buf);

            let expected_bytes_array = [
                0,  // Object ID (i)
                18, // Extension Headers Length (i)
                // {
                4, // Header Type (i)
                3, // Header Value (i)
                // }{
                5,  // Header Type (i)
                14, // Header Length (i)
                116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53,
                54, // Header Value (..)
                // }
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_subgroup_stream_object_with_even_type_extension_header() {
            let bytes_array = [
                0, // Object ID (i)
                2, // Extension Headers Length (i)
                0, // Header Type (i)
                1, // Header Value (i)
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            let object_id = 0;
            let header_type = 0;
            let value = 1;
            let header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_object,
                expected_subgroup_stream_object
            );
        }

        #[test]
        fn depacketize_subgroup_stream_object_with_odd_type_extension_header() {
            let bytes_array = [
                0,  // Object ID (i)
                16, // Extension Headers Length (i)
                1,  // Header Type (i)
                14, // Header Length (i)
                116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53,
                54, // Header Value (..)
                3,  // Object Payload Length (i)
                1, 2, 3, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur).unwrap();

            let object_id = 0;
            let header_type = 1;
            let value = vec![116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53, 54];
            let header_value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_status = None;
            let object_payload = vec![1, 2, 3];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            assert_eq!(
                depacketized_subgroup_stream_object,
                expected_subgroup_stream_object
            );
        }

        #[test]
        fn depacketize_subgroup_stream_object_with_mixed_type_extension_headers() {
            let bytes_array = [
                0,  // Object ID (i)
                18, // Extension Headers Length (i)
                // {
                4, // Header Type (i)
                3, // Header Value (i)
                // }{
                5,  // Header Type (i)
                14, // Header Length (i)
                116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53,
                54, // Header Value (..)
                // }
                0, // Object Payload Length (i)
                0, // Object Status (i)
            ];

            let object_id = 0;

            let even_header_type = 4;
            let even_value = 3;
            let even_header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(even_value));

            let odd_header_type = 5;
            let odd_value = vec![116, 114, 97, 99, 101, 73, 68, 58, 49, 50, 51, 52, 53, 54];
            let odd_header_value =
                ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(odd_value));

            let extension_headers = vec![
                ExtensionHeader::new(even_header_type, even_header_value).unwrap(),
                ExtensionHeader::new(odd_header_type, odd_header_value).unwrap(),
            ];
            let object_status = Some(ObjectStatus::Normal);
            let object_payload = vec![];

            let expected_subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            )
            .unwrap();

            println!(
                "expected_subgroup_stream_object: {:?}",
                expected_subgroup_stream_object
            );

            let mut buf: BytesMut = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur);

            println!(
                "depacketized_subgroup_stream_object: {:?}",
                depacketized_subgroup_stream_object
            );

            assert_eq!(
                depacketized_subgroup_stream_object.unwrap(),
                expected_subgroup_stream_object
            );
        }
    }

    mod failure {
        use bytes::BytesMut;
        use std::io::Cursor;

        use crate::modules::session_handlers::messages::data_streams::{
            DataStreams, object_status::ObjectStatus, subgroup_stream,
        };

        #[test]
        fn packetize_subgroup_stream_object_not_normal_and_not_empty_payload() {
            let object_id = 0;
            let extension_headers = vec![];
            let object_status = Some(ObjectStatus::EndOfTrackAndGroup);
            let object_payload = vec![0, 1, 2];

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status,
                object_payload,
            );

            assert!(subgroup_stream_object.is_err());
        }

        #[test]
        fn depacketize_subgroup_stream_object_wrong_object_status() {
            let bytes_array = [
                0, // Object ID (i)
                0, // Extension Headers Length (i)
                0, // Object Payload Length (i)
                2, // Object Status (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_subgroup_stream_object =
                subgroup_stream::Object::depacketize(&mut read_cur);

            assert!(depacketized_subgroup_stream_object.is_err());
        }
    }
}

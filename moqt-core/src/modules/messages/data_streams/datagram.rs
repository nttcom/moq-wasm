use super::extension_header::ExtensionHeader;
use crate::{
    messages::data_streams::DataStreams,
    variable_bytes::{read_fixed_length_bytes, read_variable_bytes_to_end},
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use serde::Serialize;

/// Implementation of object message per QUIC Datagram.
/// Type of Data Streams: OBJECT_DATAGRAM (0x1)
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Object {
    track_alias: u64,
    group_id: u64,
    object_id: u64,
    publisher_priority: u8,
    extension_headers_length: u64,
    extension_headers: Vec<ExtensionHeader>,
    object_payload: Vec<u8>,
}

impl Object {
    pub fn new(
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        extension_headers: Vec<ExtensionHeader>,
        object_payload: Vec<u8>,
    ) -> Result<Self> {
        // length of total byte of extension headers
        let mut extension_headers_length = 0;
        for header in &extension_headers {
            extension_headers_length += header.byte_length() as u64;
        }

        Ok(Object {
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers_length,
            extension_headers,
            object_payload,
        })
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

    pub fn extension_headers(&self) -> &Vec<ExtensionHeader> {
        &self.extension_headers
    }

    pub fn object_payload(&self) -> Vec<u8> {
        self.object_payload.clone()
    }
}

impl DataStreams for Object {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let group_id = read_variable_integer(read_cur).context("group id")?;
        let object_id = read_variable_integer(read_cur).context("object id")?;
        let publisher_priority =
            read_fixed_length_bytes(read_cur, 1).context("publisher priority")?[0];

        let extension_headers_length =
            read_variable_integer(read_cur).context("extension headers length")?;

        let mut extension_headers_vec = vec![];
        let extension_headers =
            read_fixed_length_bytes(read_cur, extension_headers_length as usize)
                .context("extension headers")?;
        let mut extension_headers_cur = std::io::Cursor::new(&extension_headers[..]);

        while extension_headers_cur.has_remaining() {
            let extension_header = ExtensionHeader::depacketize(&mut extension_headers_cur)
                .context("extension header")?;
            extension_headers_vec.push(extension_header);
        }

        let object_payload = read_variable_bytes_to_end(read_cur).context("object payload")?;

        tracing::trace!("Depacketized Datagram Object message.");

        Ok(Object {
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers_length,
            extension_headers: extension_headers_vec,
            object_payload,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        buf.extend(write_variable_integer(self.object_id));
        buf.extend(self.publisher_priority.to_be_bytes());

        buf.extend(write_variable_integer(self.extension_headers_length));
        for header in &self.extension_headers {
            header.packetize(buf);
        }

        buf.extend(&self.object_payload);

        tracing::trace!("Packetized Datagram Object message.");
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::messages::data_streams::{
            datagram,
            extension_header::{ExtensionHeader, ExtensionHeaderValue, Value, ValueWithLength},
            DataStreams,
        };
        use bytes::BytesMut;
        use std::io::Cursor;

        #[test]
        fn packetize_datagram_object_normal() {
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let extension_headers = vec![];
            let object_payload = vec![0, 1, 2];

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            datagram_object.packetize(&mut buf);

            let expected_bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Extension Headers Length (i)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_datagram_object_normal() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Extension Headers Length (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_datagram_object =
                datagram::Object::depacketize(&mut read_cur).unwrap();

            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let extension_headers = vec![];
            let object_payload = vec![0, 1, 2];

            let expected_datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_datagram_object, expected_datagram_object);
        }

        #[test]
        fn packetize_datagram_stream_object_with_even_type_extension_header() {
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let header_type = 4;
            let value = 1;
            let header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_payload = vec![0, 1, 2];

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            datagram_object.packetize(&mut buf);

            let expected_bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                2, // Extension Headers Length (i)
                4, // Header Type (i)
                1, // Header Value (i)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_datagram_stream_object_with_odd_type_extension_header() {
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let header_type = 1;
            let value = vec![1, 2, 3];
            let header_value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_payload = vec![0, 1, 2];

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            datagram_object.packetize(&mut buf);

            let expected_bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                5, // Extension Headers Length (i)
                1, // Header Type (i)
                3, // Header Value Length (i)
                1, 2, 3, // Header Value (..)
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn packetize_datagram_stream_object_with_mixed_type_extension_headers() {
            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let even_header_type = 12;
            let even_value = 1;
            let even_header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(even_value));
            let odd_header_type = 9;
            let odd_value = vec![1, 2, 3];
            let odd_header_value =
                ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(odd_value));

            let extension_headers = vec![
                ExtensionHeader::new(odd_header_type, odd_header_value).unwrap(),
                ExtensionHeader::new(even_header_type, even_header_value).unwrap(),
            ];
            let object_payload = vec![0, 1, 2];

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            datagram_object.packetize(&mut buf);

            let expected_bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                7, // Extension Headers Length (i)
                //{
                9, // Header Type (i)
                3, // Header Value Length (i)
                1, 2, 3, // Header Value (..)
                // }{
                12, // Header Type (i)
                1,  // Header Value (i)
                // }
                0, 1, 2, // Object Payload (..)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_datagram_stream_object_with_even_type_extension_header() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                2, // Extension Headers Length (i)
                4, // Header Type (i)
                1, // Header Value (i)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_datagram_object =
                datagram::Object::depacketize(&mut read_cur).unwrap();

            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let header_type = 4;
            let value = 1;
            let header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_payload = vec![0, 1, 2];

            let expected_datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_datagram_object, expected_datagram_object);
        }

        #[test]
        fn depacketize_datagram_stream_object_with_odd_type_extension_header() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                5, // Extension Headers Length (i)
                1, // Header Type (i)
                3, // Header Value Length (i)
                1, 2, 3, // Header Value (..)
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_datagram_object =
                datagram::Object::depacketize(&mut read_cur).unwrap();

            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let header_type = 1;
            let value = vec![1, 2, 3];
            let header_value = ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(value));

            let extension_headers = vec![ExtensionHeader::new(header_type, header_value).unwrap()];
            let object_payload = vec![0, 1, 2];

            let expected_datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_datagram_object, expected_datagram_object);
        }

        #[test]
        fn depacketize_datagram_stream_object_with_mixed_type_extension_headers() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                7, // Extension Headers Length (i)
                //{
                9, // Header Type (i)
                3, // Header Value Length (i)
                1, 2, 3, // Header Value (..)
                // }{
                12, // Header Type (i)
                1,  // Header Value (i)
                // }
                0, 1, 2, // Object Payload (..)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_datagram_object =
                datagram::Object::depacketize(&mut read_cur).unwrap();

            let track_alias = 1;
            let group_id = 2;
            let object_id = 3;
            let publisher_priority = 4;
            let even_header_type = 12;
            let even_value = 1;
            let even_header_value = ExtensionHeaderValue::EvenTypeValue(Value::new(even_value));
            let odd_header_type = 9;
            let odd_value = vec![1, 2, 3];
            let odd_header_value =
                ExtensionHeaderValue::OddTypeValue(ValueWithLength::new(odd_value));

            let extension_headers = vec![
                ExtensionHeader::new(odd_header_type, odd_header_value).unwrap(),
                ExtensionHeader::new(even_header_type, even_header_value).unwrap(),
            ];
            let object_payload = vec![0, 1, 2];

            let expected_datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();

            assert_eq!(depacketized_datagram_object, expected_datagram_object);
        }
    }

    mod failure {
        use bytes::BytesMut;
        use std::io::Cursor;

        use crate::messages::data_streams::{datagram, DataStreams};

        #[test]
        fn depacketize_datagram_object_with_empty_payload() {
            let bytes_array = [
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Object ID (i)
                4, // Subscriber Priority (8)
                0, // Extension Headers Length (i)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_datagram_object = datagram::Object::depacketize(&mut read_cur);

            assert!(depacketized_datagram_object.is_err());
        }
    }
}

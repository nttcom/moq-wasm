use crate::{
    messages::data_streams::DataStreams,
    variable_bytes::read_fixed_length_bytes,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{Context, Result};
use bytes::BytesMut;
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct StreamHeaderSubgroup {
    subscribe_id: u64,
    track_alias: u64,
    group_id: u64,
    subgroup_id: u64,
    publisher_priority: u8,
}

impl StreamHeaderSubgroup {
    pub fn new(
        subscribe_id: u64,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<Self> {
        Ok(StreamHeaderSubgroup {
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
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

    pub fn subgroup_id(&self) -> u64 {
        self.subgroup_id
    }

    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }
}

impl DataStreams for StreamHeaderSubgroup {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer(read_cur).context("subscribe id")?;
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let group_id = read_variable_integer(read_cur).context("group id")?;
        let subgroup_id = read_variable_integer(read_cur).context("subgroup id")?;
        let publisher_priority =
            read_fixed_length_bytes(read_cur, 1).context("publisher priority")?[0];

        tracing::trace!("Depacketized Stream Header Track message.");

        Ok(StreamHeaderSubgroup {
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(write_variable_integer(self.group_id));
        buf.extend(write_variable_integer(self.subgroup_id));
        buf.extend(self.publisher_priority.to_be_bytes());

        tracing::trace!("Packetized Stream Header Track message.");
    }
    /// Method to enable downcasting from MOQTPayload to StreamHeaderSubgroup
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;
        use std::io::Cursor;

        use crate::messages::data_streams::stream_header_subgroup::{
            DataStreams, StreamHeaderSubgroup,
        };

        #[test]
        fn packetize_stream_header_subgroup() {
            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let subgroup_id = 3;
            let publisher_priority = 4;

            let stream_header_subgroup = StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            let mut buf = BytesMut::new();
            stream_header_subgroup.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Subgroup ID (i)
                4, // Subscriber Priority (8)
            ];

            assert_eq!(buf.as_ref(), expected_bytes_array);
        }

        #[test]
        fn depacketize_stream_header_subgroup() {
            let bytes_array = [
                0, // Subscribe ID (i)
                1, // Track Alias (i)
                2, // Group ID (i)
                3, // Subgroup ID (i)
                4, // Subscriber Priority (8)
            ];
            let mut buf = BytesMut::with_capacity(bytes_array.len());
            buf.extend_from_slice(&bytes_array);
            let mut read_cur = Cursor::new(&buf[..]);
            let depacketized_stream_header_subgroup =
                StreamHeaderSubgroup::depacketize(&mut read_cur).unwrap();

            let subscribe_id = 0;
            let track_alias = 1;
            let group_id = 2;
            let subgroup_id = 3;
            let publisher_priority = 4;

            let expected_stream_header_subgroup = StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            assert_eq!(
                depacketized_stream_header_subgroup,
                expected_stream_header_subgroup
            );
        }
    }
}

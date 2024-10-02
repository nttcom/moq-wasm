use crate::{
    variable_bytes::read_fixed_length_bytes_from_buffer,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

use crate::messages::moqt_payload::MOQTPayload;

#[derive(Debug, Clone, Serialize, PartialEq)]
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

    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }
}

impl MOQTPayload for StreamHeaderSubgroup {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;
        let track_alias = read_variable_integer_from_buffer(buf).context("track alias")?;
        let group_id = read_variable_integer_from_buffer(buf).context("group id")?;
        let subgroup_id = read_variable_integer_from_buffer(buf).context("subgroup id")?;
        let publisher_priority =
            read_fixed_length_bytes_from_buffer(buf, 1).context("publisher priority")?[0];

        tracing::trace!("Depacketized Stream Header Track message.");

        Ok(StreamHeaderSubgroup {
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
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
mod success {
    use crate::messages::data_streams::stream_header_subgroup::StreamHeaderSubgroup;
    use crate::messages::moqt_payload::MOQTPayload;
    use bytes::BytesMut;

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

        let mut buf = bytes::BytesMut::new();
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
        let depacketized_stream_header_subgroup =
            StreamHeaderSubgroup::depacketize(&mut buf).unwrap();

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

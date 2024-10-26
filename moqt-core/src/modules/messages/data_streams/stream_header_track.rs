use super::DataStreams;
use crate::{
    variable_bytes::read_fixed_length_bytes,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct StreamHeaderTrack {
    subscribe_id: u64,
    track_alias: u64,
    publisher_priority: u8,
}

impl StreamHeaderTrack {
    pub fn new(subscribe_id: u64, track_alias: u64, publisher_priority: u8) -> Result<Self> {
        Ok(StreamHeaderTrack {
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

    pub fn set_subscribe_id(&mut self, subscribe_id: u64) {
        self.subscribe_id = subscribe_id;
    }

    pub fn set_track_alias(&mut self, track_alias: u64) {
        self.track_alias = track_alias;
    }
}

impl DataStreams for StreamHeaderTrack {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer(read_cur).context("subscribe id")?;
        let track_alias = read_variable_integer(read_cur).context("track alias")?;
        let publisher_priority =
            read_fixed_length_bytes(read_cur, 1).context("publisher priority")?[0];

        tracing::trace!("Depacketized Stream Header Track message.");

        Ok(StreamHeaderTrack {
            subscribe_id,
            track_alias,
            publisher_priority,
        })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(self.track_alias));
        buf.extend(self.publisher_priority.to_be_bytes());

        tracing::trace!("Packetized Stream Header Track message.");
    }
    /// Method to enable downcasting from MOQTPayload to StreamHeaderTrack
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use super::DataStreams;
    use crate::messages::data_streams::stream_header_track::StreamHeaderTrack;
    use bytes::BytesMut;
    use std::io::Cursor;

    #[test]
    fn packetize_stream_header_track() {
        let subscribe_id = 0;
        let track_alias = 1;
        let publisher_priority = 2;

        let stream_header_track =
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap();

        let mut buf = bytes::BytesMut::new();
        stream_header_track.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
            1, // Track Alias (i)
            2, // Subscriber Priority (8)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_stream_header_track() {
        let bytes_array = [
            0, // Subscribe ID (i)
            1, // Track Alias (i)
            2, // Subscriber Priority (8)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let mut read_cur = Cursor::new(&buf[..]);
        let depacketized_stream_header_track =
            StreamHeaderTrack::depacketize(&mut read_cur).unwrap();

        let subscribe_id = 0;
        let track_alias = 1;
        let publisher_priority = 2;

        let expected_stream_header_track =
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap();

        assert_eq!(
            depacketized_stream_header_track,
            expected_stream_header_track
        );
    }
}

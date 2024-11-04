use super::DataStreams;
use crate::variable_integer::{read_variable_integer, write_variable_integer};
use anyhow::{Context, Result};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct FetchHeader {
    subscribe_id: u64,
}

impl FetchHeader {
    pub fn new(subscribe_id: u64) -> Result<Self> {
        Ok(FetchHeader { subscribe_id })
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}

impl DataStreams for FetchHeader {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = read_variable_integer(read_cur).context("subscribe id")?;

        tracing::trace!("Depacketized Fetch Header message.");

        Ok(FetchHeader { subscribe_id })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));

        tracing::trace!("Packetized Fetch Header message.");
    }
    /// Method to enable downcasting from MOQTPayload to FetchHeader
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod success {
    use super::DataStreams;
    use crate::messages::data_streams::fetch_header::FetchHeader;
    use bytes::BytesMut;
    use std::io::Cursor;

    #[test]
    fn packetize_stream_header_track() {
        let subscribe_id = 0;

        let stream_header_track = FetchHeader::new(subscribe_id).unwrap();

        let mut buf = bytes::BytesMut::new();
        stream_header_track.packetize(&mut buf);

        let expected_bytes_array = [
            0, // Subscribe ID (i)
        ];

        assert_eq!(buf.as_ref(), expected_bytes_array);
    }

    #[test]
    fn depacketize_stream_header_track() {
        let bytes_array = [
            0, // Subscribe ID (i)
        ];
        let mut buf = BytesMut::with_capacity(bytes_array.len());
        buf.extend_from_slice(&bytes_array);
        let mut read_cur = Cursor::new(&buf[..]);
        let depacketized_stream_header_track = FetchHeader::depacketize(&mut read_cur).unwrap();

        let subscribe_id = 0;

        let expected_stream_header_track = FetchHeader::new(subscribe_id).unwrap();

        assert_eq!(
            depacketized_stream_header_track,
            expected_stream_header_track
        );
    }
}

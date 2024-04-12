use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    modules::variable_bytes::write_variable_bytes, variable_bytes::{read_variable_bytes_with_length_from_buffer},
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};

use super::moqt_payload::MOQTPayload;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AnnounceOk {
    track_namespace: String,
}

impl AnnounceOk {
    pub(crate) fn new(track_namespace: String) -> Self {
        Self { track_namespace }
    }
}

impl MOQTPayload for AnnounceOk {
    fn depacketize(buf: &mut bytes::BytesMut) -> Result<Self> {
        let track_namespace_length = u8::try_from(read_variable_integer_from_buffer(buf)?)
            .context("number of supported versions")?;

        let track_namespace = String::from_utf8(read_variable_bytes_with_length_from_buffer(
            buf,
            track_namespace_length as usize,
        )?)
        .context("track namespace")?;
    

        Ok(AnnounceOk { track_namespace })
    }

    fn packetize(&self, buf: &mut bytes::BytesMut) {
        // Track Namespace bytes Length
        buf.extend(write_variable_integer(self.track_namespace.len() as u64));

        // Track Namespace bytes
        buf.extend(write_variable_bytes(
            &self.track_namespace.as_bytes().to_vec(),
        ));
    }
}


#[cfg(test)]
mod success {
    use bytes::BytesMut;
    use crate::modules::messages::announce_ok_message::AnnounceOk;
    use crate::messages::moqt_payload::MOQTPayload;

    #[test]
    fn packetize() {
        let track_namespace = "test".to_string();
        let announce_ok = AnnounceOk::new(track_namespace.clone());
        let mut buf = BytesMut::new();
        announce_ok.packetize(&mut buf);

        // Track Namespace bytes Length
        // .len()の時点ではusizeでu8としてto_be_bytesされないのでu8に事前に変換する
        let mut combined_bytes = Vec::from((track_namespace.len() as u8).to_be_bytes()); 
        // Track Namespace bytes
        combined_bytes.extend(track_namespace.as_bytes().to_vec());


        assert_eq!(buf.as_ref(), combined_bytes.as_slice());
    }

    #[test]
    fn depacketize() {
        let track_namespace = "test".to_string();
        let mut buf = BytesMut::new();
        // Track Namespace bytes Length
        buf.extend((track_namespace.len() as u8).to_be_bytes());
        // Track Namespace bytes
        buf.extend(track_namespace.as_bytes().to_vec());
        
        let announce_ok = AnnounceOk::depacketize(&mut buf).unwrap();

        let expected_announce_ok = AnnounceOk::new(track_namespace.clone());


        assert_eq!(announce_ok, expected_announce_ok);
    }
}
use crate::{
    messages::moqt_payload::MOQTPayload,
    variable_bytes::{
        read_bytes_from_buffer, read_variable_bytes_from_buffer, write_variable_bytes,
    },
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, IntoPrimitive, TryFromPrimitive, Serialize, Clone, Copy, PartialEq)]
#[repr(u64)]
pub enum StatusCode {
    Unsubscribed = 0x0,
    InternalError = 0x1,
    Unauthorized = 0x2,
    TrackEnded = 0x3,
    SubscriptionEnded = 0x4,
    GoingAway = 0x5,
    Expired = 0x6,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubscribeDone {
    subscribe_id: u64,
    status_code: StatusCode,
    reason_phrase: String,
    content_exists: bool,
    final_group_id: Option<u64>,
    final_object_id: Option<u64>,
}

impl SubscribeDone {
    pub fn new(
        subscribe_id: u64,
        status_code: StatusCode,
        reason_phrase: String,
        content_exists: bool,
        final_group_id: Option<u64>,
        final_object_id: Option<u64>,
    ) -> SubscribeDone {
        SubscribeDone {
            subscribe_id,
            status_code,
            reason_phrase,
            content_exists,
            final_group_id,
            final_object_id,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    pub fn status_code(&self) -> StatusCode {
        self.status_code
    }

    pub fn reason_phrase(&self) -> &str {
        &self.reason_phrase
    }

    pub fn content_exists(&self) -> bool {
        self.content_exists
    }

    pub fn final_group_id(&self) -> Option<u64> {
        self.final_group_id
    }

    pub fn final_object_id(&self) -> Option<u64> {
        self.final_object_id
    }
}

impl MOQTPayload for SubscribeDone {
    fn depacketize(buf: &mut BytesMut) -> Result<Self> {
        let subscribe_id = read_variable_integer_from_buffer(buf).context("subscribe id")?;
        let status_code_u64 = read_variable_integer_from_buffer(buf).context("status code")?;
        let status_code = StatusCode::try_from(status_code_u64).context("status code")?;
        let reason_phrase =
            String::from_utf8(read_variable_bytes_from_buffer(buf)?).context("reason phrase")?;
        let content_exists = match read_bytes_from_buffer(buf, 1).context("content_exists")?[0] {
            0 => false,
            1 => true,
            _ => {
                // TODO: return Termination Error Code
                bail!("Invalid content_exists value: Protocol Violation");
            }
        };

        let (final_group_id, final_object_id) = match content_exists {
            true => {
                let final_group_id =
                    read_variable_integer_from_buffer(buf).context("final_group_id")?;
                let final_object_id =
                    read_variable_integer_from_buffer(buf).context("final_object_id")?;
                (Some(final_group_id), Some(final_object_id))
            }
            false => (None, None),
        };

        Ok(SubscribeDone {
            subscribe_id,
            status_code,
            reason_phrase,
            content_exists,
            final_group_id,
            final_object_id,
        })
    }
    fn packetize(&self, buf: &mut BytesMut) {
        buf.extend(write_variable_integer(self.subscribe_id));
        buf.extend(write_variable_integer(u64::from(self.status_code)));
        buf.extend(write_variable_bytes(
            &self.reason_phrase.as_bytes().to_vec(),
        ));
        buf.extend(u8::from(self.content_exists).to_be_bytes());
        if self.content_exists {
            buf.extend(write_variable_integer(self.final_group_id.unwrap()));
            buf.extend(write_variable_integer(self.final_object_id.unwrap()));
        }
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeDone
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use crate::messages::{
            control_messages::subscribe_done::{StatusCode, SubscribeDone},
            moqt_payload::MOQTPayload,
        };
        use bytes::BytesMut;
        #[test]
        fn packetize() {
            let subscribe_done = SubscribeDone::new(
                0,
                StatusCode::Unsubscribed,
                "reason".to_string(),
                false,
                None,
                None,
            );
            let mut buf = BytesMut::new();
            subscribe_done.packetize(&mut buf);

            let expected_bytes_array = [
                0, // Subscribe ID (i)
                0, // Status Code (i)
                6, // Reason Phrase (b)
                b'r', b'e', b'a', b's', b'o', b'n', // Reason Phrase (b)
                0,    // Content Exists (b)
            ];
            assert_eq!(buf.as_ref(), expected_bytes_array.as_slice());
        }
        #[test]
        fn depacketize() {
            let bytes_array = [
                0, // Subscribe ID (i)
                0, // Status Code (i)
                6, // Reason Phrase (b)
                b'r', b'e', b'a', b's', b'o', b'n', // Reason Phrase (b)
                0,    // Content Exists (b)
            ];
            let mut buf = BytesMut::new();
            buf.extend_from_slice(&bytes_array);
            let depacketized_subscribe_done = SubscribeDone::depacketize(&mut buf).unwrap();

            let expected_subscribe_done = SubscribeDone::new(
                0,
                StatusCode::Unsubscribed,
                "reason".to_string(),
                false,
                None,
                None,
            );

            assert_eq!(depacketized_subscribe_done, expected_subscribe_done);
        }
    }
}

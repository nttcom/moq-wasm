use crate::messages::moqt_payload::MOQTPayload;
use bytes::BytesMut;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
use std::any::Any;

#[derive(Debug, IntoPrimitive, TryFromPrimitive, Serialize, Clone, Copy, PartialEq)]
#[repr(u8)]
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
}

impl MOQTPayload for SubscribeDone {
    fn depacketize(buf: &mut BytesMut) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let subscribe_id = buf.get_u64();
    }
    fn packetize(&self, buf: &mut BytesMut) {
        buf.put_u64(self.subscribe_id);
    }
    /// Method to enable downcasting from MOQTPayload to SubscribeError
    fn as_any(&self) -> &dyn Any {
        self
    }
}

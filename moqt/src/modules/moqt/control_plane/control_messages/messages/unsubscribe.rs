use crate::modules::extensions::{
    buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt,
};
use bytes::BytesMut;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Unsubscribe {
    pub request_id: u64,
}

impl Unsubscribe {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let request_id = buf.try_get_varint().log_context("request id").ok()?;
        Some(Self { request_id })
    }

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        payload.put_varint(self.request_id);
        payload
    }
}

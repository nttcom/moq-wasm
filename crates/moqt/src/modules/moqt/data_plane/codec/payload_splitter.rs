use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};

use crate::modules::executor;

/// What one decode step of a subgroup stream's objects yields. `H` is the
/// draft's object head.
#[derive(Debug)]
pub(crate) enum PayloadSplit<H> {
    /// An object whose payload was buffered with its head, or a status object.
    Object {
        head: H,
        payload: Bytes,
        /// From the first byte of the object being buffered to its last.
        receive_time: Duration,
    },
    /// The head of an object whose payload follows as `Payload` pieces.
    Head { head: H, payload_len: usize },
    Payload {
        /// Shares the read buffer.
        data: Bytes,
        /// Set on the last piece of the payload: from the first byte of the
        /// object being buffered to its last.
        completed_in: Option<Duration>,
    },
}

/// Splits the objects of a subgroup stream into heads and payload pieces
/// when the payload has not been buffered with the head, and times how
/// long each object took to arrive. Each draft brings its own head decoder.
#[derive(Debug)]
pub(crate) struct PayloadSplitter {
    /// Payload bytes of the current object not yet handed out.
    payload_remaining: usize,
    /// When the first byte of the next object was buffered, on the
    /// `executor::now` clock.
    object_buffered_at: Option<Duration>,
}

impl PayloadSplitter {
    /// Starts after the stream header; `rest` is what was buffered past it.
    pub(crate) fn after_header(rest: &[u8]) -> Self {
        Self {
            payload_remaining: 0,
            object_buffered_at: (!rest.is_empty()).then(executor::now),
        }
    }

    /// Whether the stream is inside a payload whose head was handed out, so
    /// that ending it here is an error.
    pub(crate) fn inside_payload(&self) -> bool {
        self.payload_remaining > 0
    }

    /// `decode_head` advances `src` past an object head and returns it with
    /// its payload length, or returns `None` while the head is incomplete.
    pub(crate) fn decode<H, E>(
        &mut self,
        src: &mut BytesMut,
        decode_head: impl FnOnce(&mut BytesMut) -> Result<Option<(H, usize)>, E>,
    ) -> Result<Option<PayloadSplit<H>>, E> {
        if src.is_empty() {
            return Ok(None);
        }
        let buffered_at = *self.object_buffered_at.get_or_insert_with(executor::now);
        if self.payload_remaining > 0 {
            let data = src.split_to(self.payload_remaining.min(src.len())).freeze();
            self.payload_remaining -= data.len();
            let completed_in =
                (self.payload_remaining == 0).then(|| self.complete_object(buffered_at, src));
            return Ok(Some(PayloadSplit::Payload { data, completed_in }));
        }
        let Some((head, payload_len)) = decode_head(src)? else {
            return Ok(None);
        };
        if src.len() < payload_len {
            self.payload_remaining = payload_len;
            return Ok(Some(PayloadSplit::Head { head, payload_len }));
        }
        let payload = Bytes::copy_from_slice(&src[..payload_len]);
        src.advance(payload_len);
        Ok(Some(PayloadSplit::Object {
            head,
            payload,
            receive_time: self.complete_object(buffered_at, src),
        }))
    }

    /// Returns how long the object took to arrive.
    fn complete_object(&mut self, buffered_at: Duration, src: &BytesMut) -> Duration {
        let decoded_at = executor::now();
        // Bytes left over arrived with this object's last bytes.
        self.object_buffered_at = (!src.is_empty()).then_some(decoded_at);
        decoded_at.saturating_sub(buffered_at)
    }
}

pub mod h264;

use bytes::Bytes;

/// Presentation timestamp in microseconds (WebCodecs convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp(pub i64);

/// A completed frame emitted by a framer, independent of codec.
pub struct Frame {
    pub keyframe: bool,
    pub timestamp: Timestamp,
    /// One frame worth of coded bytes (a run of NAL units for H.264).
    pub payload: Bytes,
}

use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum ObjectStatus {
    Normal = 0x0,
    DoesNotExist = 0x1,
    EndOfGroup = 0x3,
    EndOfTrackAndGroup = 0x4,
    EndOfTrack = 0x5,
}

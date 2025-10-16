use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum FilterType {
    LatestGroup = 0x1,
    LatestObject = 0x2,
    AbsoluteStart = 0x3,
    AbsoluteRange = 0x4,
}

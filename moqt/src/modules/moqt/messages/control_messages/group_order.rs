use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum GroupOrder {
    Ascending = 0x1,
    Descending = 0x2,
}

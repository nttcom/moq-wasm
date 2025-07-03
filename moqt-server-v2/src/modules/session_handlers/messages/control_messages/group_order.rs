use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum GroupOrder {
    Original = 0x0, // Use the original publisher's Group Order
    Ascending = 0x1,
    Descending = 0x2,
}

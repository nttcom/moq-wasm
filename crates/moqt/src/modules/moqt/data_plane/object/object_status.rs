use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum ObjectStatus {
    Normal = 0x0,
    DoesNotExist = 0x1,
    EndOfGroup = 0x3,
    EndOfTrack = 0x4,
}

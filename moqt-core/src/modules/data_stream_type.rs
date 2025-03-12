use num_enum::{IntoPrimitive, TryFromPrimitive};

#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Clone, Copy)]
#[repr(u8)]
pub enum DataStreamType {
    ObjectDatagram = 0x1,
    ObjectDatagramStatus = 0x2,
    SubgroupHeader = 0x4,
    FetchHeader = 0x5,
}

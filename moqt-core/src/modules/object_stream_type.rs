#[derive(Debug, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Clone)]
#[repr(u8)]
pub enum ObjectStreamType {
    ObjectDatagram = 0x1,
    StreamHeaderTrack = 0x2,
    StreamHeaderSubgroup = 0x4,
}

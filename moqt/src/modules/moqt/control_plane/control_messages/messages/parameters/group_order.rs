use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::Serialize;
#[derive(Debug, Serialize, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
#[repr(u8)]
pub enum GroupOrder {
    Publisher = 0x0,
    Ascending = 0x1,
    Descending = 0x2,
}

impl GroupOrder {
    /// draft-ietf-moq-transport-14 §9.8 and §9.17: SUBSCRIBE_OK and FETCH_OK
    /// state the order the groups are delivered in and never 0x0, which only
    /// asks for the publisher's order; a publisher delivers groups in the
    /// order it produces them.
    pub fn delivered(self) -> Self {
        match self {
            Self::Publisher => Self::Ascending,
            requested => requested,
        }
    }
}

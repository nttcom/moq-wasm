use crate::modules::extensions::wire_enum::wire_enum;

wire_enum! {
    #[derive(Debug, Clone, PartialEq, Eq, Copy)]
    pub enum ObjectStatus: u8 {
        Normal = 0x0,
        DoesNotExist = 0x1,
        EndOfGroup = 0x3,
        EndOfTrack = 0x4,
    }
}

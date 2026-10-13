use crate::modules::moqt::data_plane::{
    object::{object_datagram::ObjectDatagram, subgroup::SubgroupHeader},
    stream::stream_receiver::UniStreamReceiver,
};

pub(crate) enum IncomingObject {
    StreamHeader {
        stream: UniStreamReceiver,
        header: SubgroupHeader,
    },
    Datagram(ObjectDatagram),
}

impl std::fmt::Debug for IncomingObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StreamHeader { header, .. } => f
                .debug_struct("StreamHeader")
                .field("header", header)
                .finish_non_exhaustive(),
            Self::Datagram(object) => f.debug_tuple("Datagram").field(object).finish(),
        }
    }
}

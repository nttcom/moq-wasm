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

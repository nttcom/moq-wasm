use crate::modules::moqt::data_plane::{
    object::{fetch::FetchHeader, object_datagram::ObjectDatagram, subgroup::SubgroupHeader},
    stream::stream_receiver::UniStreamReceiver,
};

pub(crate) enum IncomingObject {
    StreamHeader {
        stream: UniStreamReceiver,
        header: SubgroupHeader,
    },
    Datagram(ObjectDatagram),
    Fetch {
        stream: UniStreamReceiver,
        header: FetchHeader,
    },
}

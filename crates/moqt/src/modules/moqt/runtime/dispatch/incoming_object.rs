use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        object::{fetch::FetchHeader, object_datagram::ObjectDatagram, subgroup::SubgroupHeader},
        stream::stream_receiver::UniStreamReceiver,
    },
};

pub(crate) enum IncomingObject<T: TransportProtocol> {
    StreamHeader {
        stream: UniStreamReceiver<T>,
        header: SubgroupHeader,
    },
    Datagram(ObjectDatagram),
    Fetch {
        stream: UniStreamReceiver<T>,
        header: FetchHeader,
    },
}

use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        object::{object_datagram::ObjectDatagram, subgroup::SubgroupHeader},
        streams::stream::stream_receiver::UniStreamReceiver,
    },
};

pub(crate) enum StreamWithObject<T: TransportProtocol> {
    StreamHeader {
        stream: UniStreamReceiver<T>,
        header: SubgroupHeader,
    },
    Datagram(ObjectDatagram),
}

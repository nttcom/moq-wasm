use crate::modules::moqt::data_plane::{
    object::canonical_object::Object, stream::stream_data_receiver::StreamDataReceiver,
};

#[derive(Debug)]
pub(crate) enum IncomingObject {
    Subgroup(StreamDataReceiver),
    Datagram(Object),
}

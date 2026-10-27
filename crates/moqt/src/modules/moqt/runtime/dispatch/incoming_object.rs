use crate::modules::moqt::data_plane::{
    object::canonical_object::Object, stream::stream_data_receiver::StreamDataReceiver,
};

#[derive(Debug)]
pub(crate) enum IncomingObject {
    Subgroup(StreamDataReceiver),
    Datagram(IncomingDatagram),
}

#[derive(Debug, PartialEq)]
pub(crate) struct IncomingDatagram {
    pub(crate) object: Object,
    /// The datagram omits its priority (draft-18 §11.3.1), so the object
    /// takes the default of the subscription it reaches (§12.4).
    pub(crate) inherits_priority: bool,
}

impl IncomingDatagram {
    pub(crate) fn new(object: Object) -> Self {
        Self {
            object,
            inherits_priority: false,
        }
    }
}

impl IncomingObject {
    pub(crate) fn inherit_priority(&mut self, default_publisher_priority: u8) {
        match self {
            Self::Subgroup(receiver) => receiver.inherit_priority(default_publisher_priority),
            Self::Datagram(datagram) => {
                if datagram.inherits_priority {
                    datagram.object.publisher_priority = default_publisher_priority;
                }
            }
        }
    }
}

use crate::modules::core::data_receiver::{
    datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiver,
};

pub(crate) enum DataReceiver {
    Datagram(Box<dyn DatagramReceiver>),
    Stream(Box<dyn StreamReceiver>),
}

impl DataReceiver {
    pub(crate) fn get_track_alias(&self) -> u64 {
        match self {
            Self::Datagram(receiver) => receiver.get_track_alias(),
            Self::Stream(receiver) => receiver.get_track_alias(),
        }
    }
}

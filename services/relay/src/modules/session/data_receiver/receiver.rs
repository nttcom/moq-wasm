use crate::modules::session::data_receiver::{
    datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiverFactory,
};

pub(crate) enum DataReceiver {
    Datagram(Box<dyn DatagramReceiver>),
    Stream(Box<dyn StreamReceiverFactory>),
}

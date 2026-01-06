use anyhow::bail;

use crate::TransportProtocol;
use crate::modules::moqt::control_plane::threads::enums::StreamWithObject;
use crate::modules::moqt::data_plane::object::data_object::DataObject;
use crate::modules::moqt::data_plane::streams::stream_type::ReceiveStreamType;

#[derive(Debug)]
pub struct DatagramReceiver<T: TransportProtocol> {
    receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
}

#[async_trait::async_trait]
impl<T: TransportProtocol> ReceiveStreamType<T> for DatagramReceiver<T> {
    fn is_datagram(&self) -> bool {
        true
    }

    async fn receive(&mut self) -> anyhow::Result<DataObject> {
        let result = match self.receiver.recv().await {
            Some(object) => object,
            None => bail!("Sender has been dropped."),
        };
        match result {
            StreamWithObject::Datagram(datagram) => Ok(DataObject::ObjectDatagram(datagram)),
            _ => unreachable!("DatagramReceiver can only receive ObjectDatagram"),
        }
    }
}

impl<T: TransportProtocol> DatagramReceiver<T> {
    pub(crate) async fn new(
        receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
    ) -> Self {
        Self { receiver }
    }
}

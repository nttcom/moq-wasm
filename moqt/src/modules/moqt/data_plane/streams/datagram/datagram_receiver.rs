use anyhow::bail;

use crate::TransportProtocol;
use crate::modules::moqt::control_plane::threads::enums::StreamWithObject;
use crate::modules::moqt::data_plane::object::object_datagram::ObjectDatagram;

#[derive(Debug)]
pub struct DatagramReceiver<T: TransportProtocol> {
    pub track_alias: u64,
    receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
    first_object_datagram: Option<ObjectDatagram>,
}

impl<T: TransportProtocol> DatagramReceiver<T> {
    pub(crate) async fn new(
        first_object_datagram: ObjectDatagram,
        receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
    ) -> Self {
        let track_alias = first_object_datagram.track_alias;
        Self {
            track_alias,
            first_object_datagram: Some(first_object_datagram),
            receiver,
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<ObjectDatagram> {
        if let Some(object_datagram) = self.first_object_datagram.take() {
            return Ok(object_datagram);
        }
        let result = match self.receiver.recv().await {
            Some(object) => object,
            None => bail!("Sender dropped."),
        };
        match result {
            StreamWithObject::Datagram(datagram) => Ok(datagram),
            _ => unreachable!("DatagramReceiver can only receive ObjectDatagram"),
        }
    }
}

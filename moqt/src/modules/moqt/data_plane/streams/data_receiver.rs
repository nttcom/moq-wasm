use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::{
            models::session_context::SessionContext, threads::enums::StreamWithObject,
        },
        data_plane::{
            object::data_object::DataObject,
            streams::{
                datagram::datagram_receiver::DatagramReceiver,
                stream::stream_data_receiver::StreamDataReceiver, stream_type::ReceiveStreamType,
            },
        },
    },
};

#[derive(Debug)]
pub struct DataReceiver<T: TransportProtocol> {
    receiver: Box<dyn ReceiveStreamType<T>>,
    first_packet: Option<DataObject>,
}

impl<T: TransportProtocol> DataReceiver<T> {
    pub(crate) async fn new(
        session_context: &Arc<SessionContext<T>>,
        track_alias: u64,
    ) -> anyhow::Result<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<StreamWithObject<T>>();
        session_context
            .notification_map
            .write()
            .await
            .insert(track_alias, sender)
            .ok_or_else(|| anyhow::anyhow!("Failed to insert stream"))?;
        let result = receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
        match result {
            StreamWithObject::StreamHeader { stream, header } => {
                let data_receiver = StreamDataReceiver::new(receiver, stream).await?;
                Ok(Self {
                    receiver: Box::new(data_receiver),
                    first_packet: Some(DataObject::SubgroupHeader(header)),
                })
            }
            StreamWithObject::Datagram(object) => {
                let data_receiver = DatagramReceiver::new(receiver).await;
                Ok(Self {
                    receiver: Box::new(data_receiver),
                    first_packet: Some(DataObject::ObjectDatagram(object)),
                })
            }
        }
    }

    pub fn is_datagram(&self) -> bool {
        self.receiver.is_datagram()
    }

    pub fn track_alias(&self) -> u64 {
        let first_packet = self.first_packet.as_ref().unwrap();
        match first_packet {
            DataObject::SubgroupHeader(header) => header.track_alias,
            DataObject::ObjectDatagram(object) => object.track_alias,
            _ => panic!("Invalid first packet type"),
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<DataObject> {
        if let Some(first_packet) = self.first_packet.take() {
            return Ok(first_packet);
        }
        self.receiver.receive().await
    }
}

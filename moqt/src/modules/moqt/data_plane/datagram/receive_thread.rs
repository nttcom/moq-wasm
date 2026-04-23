use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            data_plane::{
                notification::{StreamWithObject, notify},
                object::object_datagram::ObjectDatagram,
            },
            domains::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct DatagramReceiveThread;

impl DatagramReceiveThread {
    pub(crate) fn run<T: TransportProtocol>(
        context: Arc<SessionContext<T>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Datagram Receiver")
            .spawn(async move {
                tracing::debug!("Datagram Receiver started");
                loop {
                    match context.transport_connection.receive_datagram().await {
                        Ok(mut data) => {
                            Self::on_datagram_received(&context, &mut data).await;
                        }
                        Err(_) => {
                            tracing::error!("Failed to receive datagram");
                            break;
                        }
                    }
                }
            })
            .unwrap()
    }

    async fn on_datagram_received<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        data: &mut BytesMut,
    ) -> bool {
        tracing::info!("Received datagram: {:?}", data);
        let datagram_object = match ObjectDatagram::decode(data) {
            Some(object) => object,
            None => {
                tracing::error!("Failed to depacketize datagram object");
                return false;
            }
        };
        tracing::info!("Datagram object: {:?}", datagram_object);
        notify(
            context,
            datagram_object.track_alias,
            StreamWithObject::Datagram(datagram_object),
        )
        .await;
        true
    }
}

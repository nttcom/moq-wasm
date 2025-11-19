use std::sync::Arc;

use crate::{
    DatagramObject, TransportProtocol,
    modules::{
        moqt::control_plane::models::session_context::SessionContext,
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
                    let data = context.transport_connection.receive_datagram().await;
                    if let Ok(mut data) = data {
                        tracing::info!("Received datagram: {:?}", data);
                        // TODO: Error Handling.
                        let datagram_object = match DatagramObject::depacketize(&mut data) {
                            Ok(object) => object,
                            Err(e) => {
                                tracing::error!("Failed to depacketize datagram object: {}", e);
                                break;
                            }
                        };
                        tracing::info!("Datagram object: {:?}", datagram_object);
                        if let Some(sender) = context
                            .datagram_sender_map
                            .read()
                            .await
                            .get(&datagram_object.track_alias)
                        {
                            if let Err(e) = sender.send(datagram_object) {
                                tracing::error!("Failed to send datagram object: {}", e);
                            }
                        } else {
                            tracing::warn!(
                                "No sender found for track alias: {}",
                                datagram_object.track_alias
                            );
                        }
                    } else {
                        tracing::error!("Failed to receive datagram");
                        break;
                    }
                }
            })
            .unwrap()
    }
}

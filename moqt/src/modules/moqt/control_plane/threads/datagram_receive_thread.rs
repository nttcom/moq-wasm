use std::sync::Arc;

use bytes::BytesMut;

use crate::{
    Subgroup, TransportProtocol,
    modules::{
        moqt::{
            control_plane::threads::enums::StreamWithObject,
            data_plane::{
                codec::data_frame_decoder::DataFrameDecoder,
                object::object_datagram::ObjectDatagram,
                streams::stream::stream_receiver::UniStreamReceiver,
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
                    tokio::select! {
                        data = context.transport_connection.receive_datagram() => {
                            if let Ok(mut data) = data {
                                Self::on_datagram_received(&context, &mut data).await;
                            } else {
                                tracing::error!("Failed to receive datagram");
                                break;
                            }
                        },
                        stream = context.transport_connection.accept_uni() => {
                            if let Ok(stream) = stream {
                                let stream = UniStreamReceiver::new(stream, DataFrameDecoder::new());
                                Self::on_stream_received(&context, stream).await;
                            } else {
                                tracing::error!("Failed to accept uni stream");
                                break;
                            }
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
        // TODO: Error Handling.
        let datagram_object = match ObjectDatagram::decode(data) {
            Some(object) => object,
            None => {
                tracing::error!("Failed to depacketize datagram object");
                return false;
            }
        };
        tracing::info!("Datagram object: {:?}", datagram_object);
        Self::notify(
            context,
            datagram_object.track_alias,
            StreamWithObject::Datagram(datagram_object),
        )
        .await;
        true
    }

    async fn on_stream_received<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        mut stream: UniStreamReceiver<T>,
    ) -> bool {
        let subgroup = match stream.receive().await {
            Some(Ok(subgroup)) => subgroup,
            _ => {
                tracing::error!("Stream closed before receiving data");
                return false;
            }
        };

        let subgroup_header = match subgroup {
            Subgroup::Header(header) => header,
            _ => {
                tracing::error!("First message in stream is not a subgroup header");
                return false;
            }
        };

        Self::notify(
            context,
            subgroup_header.track_alias,
            StreamWithObject::StreamHeader {
                stream,
                header: subgroup_header,
            },
        )
        .await;
        true
    }

    async fn notify<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        track_alias: u64,
        stream_with_object: StreamWithObject<T>,
    ) {
        let mut count = 0;
        loop {
            if let Some(sender) = context.notification_map.read().await.get(&track_alias) {
                if let Err(e) = sender.send(stream_with_object) {
                    tracing::warn!("Failed to notify datagram object: {}", e);
                }
                break;
            } else {
                if count > 10 {
                    tracing::error!(
                        "No sender found for track alias: {} after multiple attempts",
                        track_alias
                    );
                    break;
                }
                tracing::warn!("No sender found for track alias: {}", track_alias);
                count += 1;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

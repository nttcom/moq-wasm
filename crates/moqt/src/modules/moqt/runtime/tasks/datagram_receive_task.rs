use std::sync::Arc;

use bytes::BytesMut;
use tracing::{Instrument, Span};

use crate::{
    TransportProtocol,
    modules::executor::{self, JoinHandle},
    modules::{
        moqt::{
            data_plane::object::object_datagram::ObjectDatagram,
            domains::session_context::SessionContext,
            runtime::dispatch::{
                incoming_object::IncomingObject, subscription_notifier::SubscriptionNotifier,
            },
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct DatagramReceiveTask;

impl DatagramReceiveTask {
    pub(crate) fn run<T: TransportProtocol>(
        context: Arc<SessionContext<T>>,
        datagram_span: Span,
    ) -> JoinHandle<()> {
        executor::spawn(
            "Datagram Receiver",
            async move {
                tracing::debug!("Datagram Receiver started");
                loop {
                    match context.transport_connection.receive_datagram().await {
                        Ok(mut data) => {
                            tracing::debug!("accepted incoming datagram");
                            Self::on_datagram_received(&context, &mut data).await;
                        }
                        Err(_) => {
                            tracing::error!("Failed to receive datagram");
                            break;
                        }
                    }
                }
            }
            .instrument(datagram_span),
        )
    }

    #[tracing::instrument(level = "info", name = "on_datagram_received", skip_all)]
    async fn on_datagram_received<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        data: &mut BytesMut,
    ) -> bool {
        tracing::debug!("Received datagram: {:?}", data);
        let datagram_object = match ObjectDatagram::decode(data) {
            Some(object) => object,
            None => {
                tracing::error!("Failed to depacketize datagram object");
                return false;
            }
        };

        tracing::debug!("Datagram object: {:?}", datagram_object);
        SubscriptionNotifier::notify(
            context,
            datagram_object.track_alias,
            IncomingObject::Datagram(datagram_object),
        )
        .await;
        true
    }
}

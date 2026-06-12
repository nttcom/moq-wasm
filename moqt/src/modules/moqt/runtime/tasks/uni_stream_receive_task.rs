use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    Subgroup, TransportProtocol,
    modules::{
        moqt::{
            data_plane::{
                codec::uni_stream_decoder::{UniStreamData, UniStreamDecoder},
                stream::{fetch_data_receiver::Fetch, stream_receiver::UniStreamReceiver},
            },
            domains::session_context::SessionContext,
            runtime::dispatch::{
                fetch_notifier::FetchNotifier, incoming_object::IncomingObject,
                subscription_notifier::SubscriptionNotifier,
            },
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct UniStreamReceiveTask;

impl UniStreamReceiveTask {
    pub(crate) fn run<T: TransportProtocol>(
        context: Arc<SessionContext<T>>,
        stream_span: Span,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Uni Stream Receiver")
            .spawn(
                async move {
                    tracing::debug!("Uni Stream Receiver started");
                    loop {
                        match context.transport_connection.accept_uni().await {
                            Ok(stream) => {
                                tracing::debug!("accepted incoming uni stream");
                                let stream =
                                    UniStreamReceiver::new(stream, UniStreamDecoder::new());
                                Self::on_stream_received(&context, stream).await;
                            }
                            Err(_) => {
                                tracing::error!("Failed to accept uni stream");
                                break;
                            }
                        }
                    }
                }
                .instrument(stream_span),
            )
            .unwrap()
    }

    #[tracing::instrument(level = "info", name = "on_stream_received", skip_all)]
    async fn on_stream_received<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        mut stream: UniStreamReceiver<T>,
    ) -> bool {
        let stream_data = match stream.receive().await {
            Ok(Some(stream_data)) => stream_data,
            Ok(None) => {
                tracing::info!("Stream closed before receiving data");
                return false;
            }
            Err(_) => {
                tracing::error!("Stream closed before receiving data");
                return false;
            }
        };

        match stream_data {
            UniStreamData::Subgroup(Subgroup::Header(header)) => {
                tracing::debug!(
                    track_alias = header.track_alias,
                    group_id = header.group_id,
                    "received subgroup header"
                );
                SubscriptionNotifier::notify(
                    context,
                    header.track_alias,
                    IncomingObject::StreamHeader { stream, header },
                )
                .await;
            }
            UniStreamData::Fetch(Fetch::Header(header)) => {
                FetchNotifier::notify(
                    context,
                    header.request_id,
                    IncomingObject::Fetch { stream, header },
                )
                .await;
            }
            _ => {
                tracing::error!("First message in stream is not a header");
                return false;
            }
        }
        true
    }
}

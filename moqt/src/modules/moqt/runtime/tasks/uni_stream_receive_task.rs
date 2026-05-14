use std::sync::Arc;

use tracing::{Instrument, Span};

use crate::{
    Subgroup, TransportProtocol,
    modules::{
        moqt::{
            data_plane::{
                codec::subgroup_decoder::SubgroupDecoder,
                streams::stream::stream_receiver::UniStreamReceiver,
            },
            domains::session_context::SessionContext,
            runtime::dispatch::{
                incoming_track_data::IncomingTrackData, subscription_notifier::SubscriptionNotifier,
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
                                let stream = UniStreamReceiver::new(stream, SubgroupDecoder::new());
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

        SubscriptionNotifier::notify(
            context,
            subgroup_header.track_alias,
            IncomingTrackData::StreamHeader {
                stream,
                header: subgroup_header,
            },
        )
        .await;
        true
    }
}

use std::sync::Arc;

use crate::{
    Subgroup, TransportProtocol,
    modules::{
        moqt::{
            data_plane::{
                codec::subgroup_decoder::SubgroupDecoder,
                notification::{StreamWithObject, notify},
                stream::receiver::UniStreamReceiver,
            },
            domains::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct StreamReceiveThread;

impl StreamReceiveThread {
    pub(crate) fn run<T: TransportProtocol>(
        context: Arc<SessionContext<T>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Stream Receiver")
            .spawn(async move {
                tracing::debug!("Stream Receiver started");
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
            })
            .unwrap()
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

        notify(
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
}

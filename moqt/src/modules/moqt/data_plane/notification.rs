use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        data_plane::{
            object::{object_datagram::ObjectDatagram, subgroup::SubgroupHeader},
            stream::receiver::UniStreamReceiver,
        },
        domains::session_context::SessionContext,
    },
};

pub(crate) enum StreamWithObject<T: TransportProtocol> {
    StreamHeader {
        stream: UniStreamReceiver<T>,
        header: SubgroupHeader,
    },
    Datagram(ObjectDatagram),
}

pub(crate) async fn notify<T: TransportProtocol>(
    context: &Arc<SessionContext<T>>,
    track_alias: u64,
    stream_with_object: StreamWithObject<T>,
) {
    let mut count = 0;
    loop {
        if let Some(sender) = context.notification_map.read().await.get(&track_alias) {
            if let Err(e) = sender.send(stream_with_object) {
                tracing::warn!("Failed to notify: {}", e);
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

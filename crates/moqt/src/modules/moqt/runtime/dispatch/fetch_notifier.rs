use crate::modules::moqt::{
    data_plane::stream::fetch_data_receiver::FetchDataReceiver, domains::connection::Connection,
};

pub(crate) struct FetchNotifier;

impl FetchNotifier {
    #[tracing::instrument(
        level = "info",
        name = "moqt.fetch_notifier.notify",
        skip_all,
        fields(request_id = request_id)
    )]
    pub(crate) async fn notify(
        connection: &Connection,
        request_id: u64,
        fetch_data_receiver: FetchDataReceiver,
    ) {
        // Draft-14 §9.16.3: a FETCH response is delivered on a single stream.
        let sender = connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .remove(&request_id);
        if let Some(sender) = sender {
            if let Err(e) = sender.send(fetch_data_receiver) {
                tracing::warn!("Failed to notify fetch stream: {}", e);
            }
        } else {
            tracing::error!("No sender found for request_id: {}", request_id);
        }
    }
}

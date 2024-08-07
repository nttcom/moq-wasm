use anyhow::Result;

use crate::{
    modules::{
        messages::unannounce_message::UnAnnounceMessage,
        track_namespace_manager_repository::TrackNamespaceManagerRepository,
    },
    MOQTClient,
};

pub(crate) async fn unannounce_handler(
    unannounce_message: UnAnnounceMessage,
    _client: &mut MOQTClient, // 未実装のため_をつけている
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<()> {
    tracing::info!("unannounce_handler!");

    tracing::info!(
        "unannounce_handler: track_namespace: \"{}\"",
        unannounce_message.track_namespace()
    );

    // announceされたTrack Namespaceを削除
    let delete_result = track_namespace_manager_repository
        .delete_publisher(unannounce_message.track_namespace())
        .await;

    match delete_result {
        // TODO: 接続しているクライアントに対して、unannounceされたことを通知する
        Ok(_) => Ok(()),
        Err(err) => {
            tracing::info!("unannounce_handler: err: {:?}", err.to_string());

            Ok(())
        }
    }
}

use anyhow::Result;

use crate::{
    modules::{
        messages::unannounce_message::UnAnnounceMessage,
        track_manager_repository::TrackManagerRepository,
    },
    MOQTClient,
};

pub(crate) async fn unannounce_handler(
    unannounce_message: UnAnnounceMessage,
    client: &mut MOQTClient,
    track_manager_repository: &mut dyn TrackManagerRepository,
) -> Result<()> {
    tracing::info!("unannounce_handler!");

    tracing::info!(
        "unannounce_handler: track_namespace: \"{}\"",
        unannounce_message.track_namespace()
    );

    // announceされたTrack Namespaceを記録から削除
    let delete_result = track_manager_repository
        .delete(unannounce_message.track_namespace())
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

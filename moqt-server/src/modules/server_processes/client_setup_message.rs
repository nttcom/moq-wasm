use bytes::BytesMut;

use crate::modules::handlers::server_setup_handler::setup_handler;
use anyhow::{bail, Result};
use moqt_core::{
    constants::UnderlayType,
    messages::{control_messages::client_setup::ClientSetup, moqt_payload::MOQTPayload},
    MOQTClient, TrackNamespaceManagerRepository,
};

pub(crate) async fn process_client_setup_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    underlay_type: UnderlayType,
    write_buf: &mut BytesMut,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
) -> Result<()> {
    let client_setup_message = match ClientSetup::depacketize(payload_buf) {
        Ok(client_setup_message) => client_setup_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let server_setup_message = setup_handler(
        client_setup_message,
        underlay_type,
        client,
        track_namespace_manager_repository,
    )
    .await?;

    server_setup_message.packetize(write_buf);

    Ok(())
}

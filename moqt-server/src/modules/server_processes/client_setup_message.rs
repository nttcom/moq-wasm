use bytes::BytesMut;

use crate::modules::handlers::server_setup_handler::setup_handler;
use anyhow::{bail, Result};
use moqt_core::{
    constants::UnderlayType,
    messages::{client_setup::ClientSetup, moqt_payload::MOQTPayload},
    moqt_client::MOQTClientStatus,
    MOQTClient,
};

pub(crate) fn process_client_setup_message(
    payload_buf: &mut BytesMut,
    client: &mut MOQTClient,
    underlay_type: UnderlayType,
    write_buf: &mut BytesMut,
) -> Result<()> {
    if client.status() != MOQTClientStatus::Connected {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        bail!(message);
    }
    let client_setup_message = match ClientSetup::depacketize(payload_buf) {
        Ok(client_setup_message) => client_setup_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    let server_setup_message = setup_handler(client_setup_message, underlay_type, client)?;

    server_setup_message.packetize(write_buf);

    Ok(())
}

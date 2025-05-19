use anyhow::{Result, bail};
use bytes::BytesMut;

use moqt_core::messages::{
    control_messages::announce_error::AnnounceError, moqt_payload::MOQTPayload,
};

pub(crate) async fn process_announce_error_message(payload_buf: &mut BytesMut) -> Result<()> {
    let announce_error_message = match AnnounceError::depacketize(payload_buf) {
        Ok(announce_error_message) => announce_error_message,
        Err(err) => {
            tracing::error!("{:#?}", err);
            bail!(err.to_string());
        }
    };

    tracing::warn!("announce_error_message: {:#?}", announce_error_message);

    Ok(())
}

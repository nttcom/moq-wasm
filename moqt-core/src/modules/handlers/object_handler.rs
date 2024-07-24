use anyhow::Result;
use messages::object_message::ObjectMessage;

pub(crate) fn object_handler(object_message: ObjectMessage, client: &mut MOQTClient) -> Result<()> {
    tracing::info!("object_handler");

    Ok(())
}

use crate::modules::{
    constants::UnderlayType, messages::setup_parameters::SetupParameter,
    moqt_client::MOQTClientStatus,
};
use anyhow::{bail, ensure, Result};
use bytes::BytesMut;

use crate::modules::{
    messages::{
        payload::Payload,
        setup_message::{ClientSetupMessage, ServerSetupMessage},
    },
    moqt_client::MOQTClient,
};

pub(crate) fn setup_handler(
    buf: &mut BytesMut,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
) -> Result<ServerSetupMessage> {
    tracing::info!("setup_handler");

    ensure!(
        client.status() == MOQTClientStatus::Connected,
        "Invalid timing"
    );

    let client_setup_message = ClientSetupMessage::depacketize(buf)?;

    let tmp_supported_version = 1;

    if !client_setup_message
        .supported_versions
        .iter()
        .any(|v| *v == tmp_supported_version)
    {
        bail!("Supported version is not included");
    }

    for setup_parameter in &client_setup_message.setup_parameters {
        match setup_parameter {
            SetupParameter::RoleParameter(role) => {
                client.set_role(role.value)?;
            }
            SetupParameter::PathParameter(_) => {
                if underlay_type == UnderlayType::WebTransport {
                    bail!("PATH parameter is not allowed on WebTransport.");
                }
            }
            SetupParameter::Unknown(v) => {
                tracing::info!("Ignore unknown SETUP parameter {}", v);
            }
        }
    }

    if let None = client.role() {
        bail!("Role parameter is required in SETUP parameter from client.");
    }

    let server_setup_message = ServerSetupMessage::new(tmp_supported_version, vec![]);
    // Connected -> Setup
    client.update_status(MOQTClientStatus::SetUp);

    tracing::info!("setup_handler completed. {:#?}", client);

    Ok(server_setup_message)
}

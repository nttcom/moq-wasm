use crate::constants;
use anyhow::{bail, Result};

use moqt_core::{
    constants::UnderlayType,
    messages::control_messages::{
        client_setup::ClientSetup,
        server_setup::ServerSetup,
        setup_parameters::SetupParameter,
        setup_parameters::{RoleCase, RoleParameter},
    },
    moqt_client::MOQTClientStatus,
    MOQTClient,
};

pub(crate) fn setup_handler(
    client_setup_message: ClientSetup,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
) -> Result<ServerSetup> {
    tracing::trace!("setup_handler start.");

    tracing::debug!("client_setup_message: {:#?}", client_setup_message);

    tracing::debug!(
        "supported_versions: {:#x?}",
        client_setup_message.supported_versions
    );

    if !client_setup_message
        .supported_versions
        .iter()
        .any(|v| *v == constants::MOQ_TRANSPORT_VERSION)
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
                tracing::warn!("Ignore unknown SETUP parameter {}", v);
            }
        }
    }

    if client.role().is_none() {
        bail!("Role parameter is required in SETUP parameter from client.");
    }

    // Create a setup parameter with role set to 3 and assign it.
    // Normally, the server should determine the role here, but for now, let's set it to 3.
    let role_parameter = SetupParameter::RoleParameter(RoleParameter::new(RoleCase::Both));
    let server_setup_message =
        ServerSetup::new(constants::MOQ_TRANSPORT_VERSION, vec![role_parameter]);
    // State: Connected -> Setup
    client.update_status(MOQTClientStatus::SetUp);

    tracing::trace!("setup_handler complete.");

    Ok(server_setup_message)
}

#[cfg(test)]
mod success {
    use std::vec;

    use crate::{constants, modules::handlers::server_setup_handler::setup_handler};
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{PathParameter, RoleCase, RoleParameter, SetupParameter},
    };
    use moqt_core::moqt_client::MOQTClient;

    #[test]
    fn only_role() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::RoleParameter(RoleParameter::new(
            RoleCase::Injection,
        ))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_ok());
        let _server_setup_message = server_setup_message.unwrap(); // TODO: Not implemented yet
    }

    #[test]
    fn role_and_path_on_quic() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![
            SetupParameter::RoleParameter(RoleParameter::new(RoleCase::Injection)),
            SetupParameter::PathParameter(PathParameter::new(String::from("test"))),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_ok());
        let _server_setup_message = server_setup_message.unwrap(); // TODO: Not implemented yet
    }
}

#[cfg(test)]
mod failure {
    use std::vec;

    use crate::{constants, modules::handlers::server_setup_handler::setup_handler};
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{PathParameter, RoleCase, RoleParameter, SetupParameter},
    };
    use moqt_core::moqt_client::MOQTClient;

    #[test]
    fn no_role_parameter() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_err());
    }

    #[test]
    fn include_path_on_wt() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::PathParameter(PathParameter::new(
            String::from("test"),
        ))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_err());
    }

    #[test]
    fn include_only_path_on_quic() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::PathParameter(PathParameter::new(
            String::from("test"),
        ))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_err());
    }

    #[test]
    fn include_unsupported_version() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::RoleParameter(RoleParameter::new(
            RoleCase::Delivery,
        ))];

        let unsupported_version = 8888;
        let client_setup_message = ClientSetup::new(vec![unsupported_version], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert_ne!(unsupported_version, constants::MOQ_TRANSPORT_VERSION); // assert unsupported_version is unsupport
        assert!(server_setup_message.is_err());
    }

    #[test]
    fn include_unknown_parameter() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![
            SetupParameter::RoleParameter(RoleParameter::new(RoleCase::Injection)),
            SetupParameter::Unknown(0),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        let server_setup_message = setup_handler(client_setup_message, underlay_type, &mut client);

        assert!(server_setup_message.is_ok());
    }
}

use crate::constants;
use anyhow::{bail, Result};
use moqt_core::pubsub_relation_manager_repository::PubSubRelationManagerRepository;
use moqt_core::{
    constants::UnderlayType,
    messages::control_messages::{
        client_setup::ClientSetup,
        server_setup::ServerSetup,
        setup_parameters::SetupParameter,
        setup_parameters::{Role, RoleCase},
    },
    moqt_client::MOQTClientStatus,
    MOQTClient,
};

pub(crate) async fn setup_handler(
    client_setup_message: ClientSetup,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
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

    let mut max_subscribe_id: u64 = 0;

    for setup_parameter in &client_setup_message.setup_parameters {
        match setup_parameter {
            SetupParameter::Role(param) => {
                client.set_role(param.value)?;
            }
            SetupParameter::Path(_) => {
                if underlay_type == UnderlayType::WebTransport {
                    bail!("PATH parameter is not allowed on WebTransport.");
                }
            }
            SetupParameter::MaxSubscribeID(param) => {
                max_subscribe_id = param.value;
            }
            SetupParameter::Unknown(v) => {
                tracing::warn!("Ignore unknown SETUP parameter {}", v);
            }
        }
    }

    if client.role() == Some(RoleCase::Subscriber) {
        // Generate producer that manages namespaces and subscriptions with subscribers.
        // FIXME: max_subscribe_id for subscriber is fixed at 100 for now.
        pubsub_relation_manager_repository
            .setup_subscriber(100, client.id)
            .await?;
    } else if client.role() == Some(RoleCase::Publisher) {
        // Generate consumer that manages namespaces and subscriptions with producers.
        pubsub_relation_manager_repository
            .setup_publisher(max_subscribe_id, client.id)
            .await?;
    } else if client.role().is_none() {
        bail!("Role parameter is required in SETUP parameter from client.");
    }

    // Create a setup parameter with role set to 3 and assign it.
    // Normally, the server should determine the role here, but for now, let's set it to 3.
    let role_parameter = SetupParameter::Role(Role::new(RoleCase::PubSub));
    let server_setup_message =
        ServerSetup::new(constants::MOQ_TRANSPORT_VERSION, vec![role_parameter]);
    // State: Connected -> Setup
    client.update_status(MOQTClientStatus::SetUp);

    tracing::trace!("setup_handler complete.");

    Ok(server_setup_message)
}

#[cfg(test)]
mod success {
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use crate::{constants, modules::handlers::server_setup_handler::setup_handler};
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{Path, Role, RoleCase, SetupParameter},
    };
    use moqt_core::moqt_client::MOQTClient;
    use std::vec;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn only_role() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::Role(Role::new(RoleCase::Publisher))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_ok());
        let _server_setup_message = server_setup_message.unwrap(); // TODO: Not implemented yet
    }

    #[tokio::test]
    async fn role_and_path_on_quic() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![
            SetupParameter::Role(Role::new(RoleCase::Publisher)),
            SetupParameter::Path(Path::new(String::from("test"))),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_ok());
        let _server_setup_message = server_setup_message.unwrap(); // TODO: Not implemented yet
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::relation_manager::{
        commands::PubSubRelationCommand, interface::PubSubRelationManagerInterface,
        manager::pubsub_relation_manager,
    };
    use crate::{constants, modules::handlers::server_setup_handler::setup_handler};
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{Path, Role, RoleCase, SetupParameter},
    };
    use moqt_core::moqt_client::MOQTClient;
    use std::vec;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn no_role_parameter() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_err());
    }

    #[tokio::test]
    async fn include_path_on_wt() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::Path(Path::new(String::from("test")))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_err());
    }

    #[tokio::test]
    async fn include_only_path_on_quic() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::Path(Path::new(String::from("test")))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_err());
    }

    #[tokio::test]
    async fn include_unsupported_version() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![SetupParameter::Role(Role::new(RoleCase::Subscriber))];

        let unsupported_version = 8888;
        let client_setup_message = ClientSetup::new(vec![unsupported_version], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert_ne!(unsupported_version, constants::MOQ_TRANSPORT_VERSION); // assert unsupported_version is unsupport
        assert!(server_setup_message.is_err());
    }

    #[tokio::test]
    async fn include_unknown_parameter() {
        let mut client = MOQTClient::new(33);
        let setup_parameters = vec![
            SetupParameter::Role(Role::new(RoleCase::Publisher)),
            SetupParameter::Unknown(0),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerInterface
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerInterface =
            PubSubRelationManagerInterface::new(track_namespace_tx);

        let server_setup_message = setup_handler(
            client_setup_message,
            underlay_type,
            &mut client,
            &mut pubsub_relation_manager,
        )
        .await;

        assert!(server_setup_message.is_ok());
    }
}

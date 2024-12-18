use anyhow::{bail, Result};
use moqt_core::{
    constants::UnderlayType,
    messages::control_messages::{
        client_setup::ClientSetup,
        server_setup::ServerSetup,
        setup_parameters::MaxSubscribeID,
        setup_parameters::SetupParameter,
        setup_parameters::{Role, RoleCase},
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

use crate::{
    constants,
    modules::moqt_client::{MOQTClient, MOQTClientStatus},
};

fn is_requested_version_supported(supported_versions: Vec<u32>) -> bool {
    supported_versions
        .iter()
        .any(|v| *v == constants::MOQ_TRANSPORT_VERSION)
}

fn handle_setup_parameter(
    client: &mut MOQTClient,
    setup_parameters: Vec<SetupParameter>,
    underlay_type: UnderlayType,
) -> Result<u64> {
    let mut max_subscribe_id: u64 = 0;
    for setup_parameter in &setup_parameters {
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
    Ok(max_subscribe_id)
}

async fn setup_subscription_node(
    client: &MOQTClient,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    upstream_max_subscribe_id: u64,
    downstream_max_subscribe_id: u64,
) -> Result<()> {
    match client.role() {
        Some(RoleCase::Publisher) => {
            // Generate consumer that manages namespaces and subscriptions with producers.
            pubsub_relation_manager_repository
                .setup_publisher(upstream_max_subscribe_id, client.id())
                .await?;
        }
        Some(RoleCase::Subscriber) => {
            // Generate producer that manages namespaces and subscriptions with subscribers.
            pubsub_relation_manager_repository
                .setup_subscriber(downstream_max_subscribe_id, client.id())
                .await?;
        }
        Some(RoleCase::PubSub) => {
            // Generate producer and consumer that manages namespaces and subscriptions with publishers and subscribers.
            pubsub_relation_manager_repository
                .setup_publisher(upstream_max_subscribe_id, client.id())
                .await?;
            pubsub_relation_manager_repository
                .setup_subscriber(downstream_max_subscribe_id, client.id())
                .await?;
        }
        None => {
            bail!("Role parameter is required in SETUP parameter from client.");
        }
    }
    Ok(())
}

fn create_setup_parameters(
    client: &MOQTClient,
    downstream_max_subscribe_id: u64,
) -> Vec<SetupParameter> {
    let mut setup_parameters = vec![];

    // Create a setup parameter with role set to 3 and assign it.
    // Normally, the server should determine the role here, but for now, let's set it to 3.
    let role_parameter = SetupParameter::Role(Role::new(RoleCase::PubSub));
    setup_parameters.push(role_parameter);

    if client.role() == Some(RoleCase::Subscriber) || client.role() == Some(RoleCase::PubSub) {
        let max_subscribe_id_parameter =
            SetupParameter::MaxSubscribeID(MaxSubscribeID::new(downstream_max_subscribe_id));
        setup_parameters.push(max_subscribe_id_parameter);
    }
    let max_subscribe_id_parameter =
        SetupParameter::MaxSubscribeID(MaxSubscribeID::new(downstream_max_subscribe_id));
    setup_parameters.push(max_subscribe_id_parameter);

    setup_parameters
}

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

    if !is_requested_version_supported(client_setup_message.supported_versions) {
        bail!("Supported version is not included");
    }

    // If max_subscribe_id is not included in the CLIENT_SETUP message, upstream_max_subscribe_id is set to 0.
    let upstream_max_subscribe_id: u64 =
        handle_setup_parameter(client, client_setup_message.setup_parameters, underlay_type)?;

    // FIXME: downstream_max_subscribe_id for subscriber is fixed at 100 for now.
    let downstream_max_subscribe_id: u64 = 100;

    setup_subscription_node(
        client,
        pubsub_relation_manager_repository,
        upstream_max_subscribe_id,
        downstream_max_subscribe_id,
    )
    .await?;

    let setup_parameters = create_setup_parameters(client, downstream_max_subscribe_id);
    let server_setup_message = ServerSetup::new(constants::MOQ_TRANSPORT_VERSION, setup_parameters);
    // State: Connected -> Setup
    client.update_status(MOQTClientStatus::SetUp);

    tracing::trace!("setup_handler complete.");

    Ok(server_setup_message)
}

#[cfg(test)]
mod success {
    use crate::modules::moqt_client::MOQTClient;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::server_processes::senders;
    use crate::{
        constants,
        modules::message_handlers::control_message::handlers::server_setup_handler::setup_handler,
    };
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{Path, Role, RoleCase, SetupParameter},
    };
    use std::vec;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn only_role() {
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![SetupParameter::Role(Role::new(RoleCase::Publisher))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![
            SetupParameter::Role(Role::new(RoleCase::Publisher)),
            SetupParameter::Path(Path::new(String::from("test"))),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
    use super::setup_handler;
    use crate::constants;
    use crate::modules::moqt_client::MOQTClient;
    use crate::modules::pubsub_relation_manager::{
        commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        wrapper::PubSubRelationManagerWrapper,
    };
    use crate::modules::server_processes::senders;
    use moqt_core::messages::control_messages::{
        client_setup::ClientSetup,
        setup_parameters::{Path, Role, RoleCase, SetupParameter},
    };
    use std::vec;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn no_role_parameter() {
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![SetupParameter::Path(Path::new(String::from("test")))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![SetupParameter::Path(Path::new(String::from("test")))];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::QUIC;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![SetupParameter::Role(Role::new(RoleCase::Subscriber))];

        let unsupported_version = 8888;
        let client_setup_message = ClientSetup::new(vec![unsupported_version], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(33, senders_mock);
        let setup_parameters = vec![
            SetupParameter::Role(Role::new(RoleCase::Publisher)),
            SetupParameter::Unknown(0),
        ];
        let client_setup_message =
            ClientSetup::new(vec![constants::MOQ_TRANSPORT_VERSION], setup_parameters);
        let underlay_type = crate::constants::UnderlayType::WebTransport;

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

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

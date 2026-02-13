use crate::modules::{
    control_message_dispatcher::ControlMessageDispatcher,
    moqt_client::MOQTClient,
    signal_dispatcher::{DataStreamThreadSignal, SignalDispatcher, TerminateReason},
};
use anyhow::Result;
use moqt_core::{
    messages::control_messages::{
        subscribe_done::{StatusCode, SubscribeDone},
        unsubscribe::Unsubscribe,
    },
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};

pub(crate) async fn unsubscribe_handler(
    unsubscribe_message: Unsubscribe,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
    client: &MOQTClient,
) -> Result<()> {
    tracing::trace!("unsubscribe_handler start.");
    tracing::debug!("unsubscribe_message: {:#?}", unsubscribe_message);

    let downstream_session_id = client.id();
    let downstream_subscribe_id = unsubscribe_message.subscribe_id();
    let (upstream_session_id, upstream_subscribe_id) = pubsub_relation_manager_repository
        .get_related_publisher(downstream_session_id, downstream_subscribe_id)
        .await?;

    terminate_downstream_subgroup_forwarders(
        pubsub_relation_manager_repository,
        client,
        downstream_session_id,
        downstream_subscribe_id,
    )
    .await;

    // 1. Delete Subscription from PubSubRelationManager
    pubsub_relation_manager_repository
        .delete_downstream_subscription(downstream_session_id, downstream_subscribe_id)
        .await?;
    pubsub_relation_manager_repository
        .delete_pubsub_relation(
            upstream_session_id,
            upstream_subscribe_id,
            downstream_session_id,
            downstream_subscribe_id,
        )
        .await?;

    // 2. Response SUBSCRIBE_DONE to Client
    let subscribe_done_message = Box::new(SubscribeDone::new(
        downstream_subscribe_id,
        StatusCode::Unsubscribed,
        "Unsubscribe from subscriber".to_string(),
        false,
        None,
        None,
    ));
    control_message_dispatcher
        .transfer_message_to_control_message_sender_thread(client.id(), subscribe_done_message)
        .await?;

    // 3. If the number of DownStream Subscriptions is zero, send UNSUBSCRIBE to the Original Publisher.
    let downstream_subscribers = pubsub_relation_manager_repository
        .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
        .await?;
    if downstream_subscribers.is_empty() {
        let unsubscribe_message = Box::new(Unsubscribe::new(upstream_subscribe_id));
        control_message_dispatcher
            .transfer_message_to_control_message_sender_thread(
                upstream_session_id,
                unsubscribe_message,
            )
            .await?;
    }

    tracing::trace!("unsubscribe_handler complete.");
    Ok(())
}

async fn terminate_downstream_subgroup_forwarders(
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    client: &MOQTClient,
    downstream_session_id: usize,
    downstream_subscribe_id: u64,
) {
    let group_ids = match pubsub_relation_manager_repository
        .get_downstream_group_ids_for_subscription(downstream_session_id, downstream_subscribe_id)
        .await
    {
        Ok(group_ids) => group_ids,
        Err(err) => {
            tracing::warn!(
                "skip forwarder termination: failed to get downstream groups (session_id: {}, subscribe_id: {}): {:?}",
                downstream_session_id,
                downstream_subscribe_id,
                err
            );
            return;
        }
    };

    if group_ids.is_empty() {
        return;
    }

    let signal_dispatcher = SignalDispatcher::new(client.senders().signal_dispatch_tx().clone());
    let signal = Box::new(DataStreamThreadSignal::Terminate(
        TerminateReason::SessionClosed,
    ));

    for group_id in group_ids {
        let subgroup_ids = match pubsub_relation_manager_repository
            .get_downstream_subgroup_ids_for_group(
                downstream_session_id,
                downstream_subscribe_id,
                group_id,
            )
            .await
        {
            Ok(subgroup_ids) => subgroup_ids,
            Err(err) => {
                tracing::warn!(
                    "skip forwarder termination: failed to get downstream subgroups (session_id: {}, subscribe_id: {}, group_id: {}): {:?}",
                    downstream_session_id,
                    downstream_subscribe_id,
                    group_id,
                    err
                );
                continue;
            }
        };

        for subgroup_id in subgroup_ids {
            let stream_id = match pubsub_relation_manager_repository
                .get_downstream_stream_id_for_subgroup(
                    downstream_session_id,
                    downstream_subscribe_id,
                    group_id,
                    subgroup_id,
                )
                .await
            {
                Ok(Some(stream_id)) => stream_id,
                Ok(None) => {
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        "skip forwarder termination: failed to get stream_id (session_id: {}, subscribe_id: {}, group_id: {}, subgroup_id: {}): {:?}",
                        downstream_session_id,
                        downstream_subscribe_id,
                        group_id,
                        subgroup_id,
                        err
                    );
                    continue;
                }
            };

            if let Err(err) = signal_dispatcher
                .transfer_signal_to_data_stream_thread(
                    downstream_session_id,
                    stream_id,
                    signal.clone(),
                )
                .await
            {
                tracing::warn!(
                    "skip forwarder termination: failed to send terminate signal (session_id: {}, subscribe_id: {}, stream_id: {}): {:?}",
                    downstream_session_id,
                    downstream_subscribe_id,
                    stream_id,
                    err
                );
            }
        }
    }
}

#[cfg(test)]
mod success {
    use moqt_core::messages::control_messages::unsubscribe::Unsubscribe;

    use super::unsubscribe_handler;
    use crate::SenderToOpenSubscription;
    use crate::modules::{
        control_message_dispatcher::{
            ControlMessageDispatchCommand, ControlMessageDispatcher, control_message_dispatcher,
        },
        moqt_client::MOQTClient,
        object_cache_storage::{
            commands::ObjectCacheStorageCommand, storage::object_cache_storage,
            wrapper::ObjectCacheStorageWrapper,
        },
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
            wrapper::PubSubRelationManagerWrapper,
        },
        server_processes::senders,
    };
    use moqt_core::{
        messages::{
            control_messages::{group_order::GroupOrder, subscribe::FilterType},
            moqt_payload::MOQTPayload,
        },
        pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    };
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::{Mutex, mpsc};

    async fn spawn_pubsub_relation_manager() -> PubSubRelationManagerWrapper {
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let pubsub_relation_manager_wrapper: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);
        pubsub_relation_manager_wrapper
    }

    async fn spawn_control_message_dispatcher() -> ControlMessageDispatcher {
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);
        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());
        control_message_dispatcher
    }

    async fn spawn_object_cache_storage() -> ObjectCacheStorageWrapper {
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        ObjectCacheStorageWrapper::new(cache_tx)
    }

    async fn create_start_fowarder_txes() -> Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> {
        let start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> =
            Arc::new(Mutex::new(HashMap::new()));
        start_forwarder_txes
    }

    async fn create_moqt_client() -> MOQTClient {
        let downstream_session_id = 10;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        MOQTClient::new(downstream_session_id, senders_mock)
    }

    async fn initialize() -> (
        PubSubRelationManagerWrapper,
        ControlMessageDispatcher,
        ObjectCacheStorageWrapper,
        Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
        MOQTClient,
    ) {
        let control_message_dispatcher = spawn_control_message_dispatcher().await;
        let object_cache_storage_wrapper = spawn_object_cache_storage().await;
        let pubsub_relation_manager_wrapper = spawn_pubsub_relation_manager().await;
        let start_forwarder_txes = create_start_fowarder_txes().await;
        let client = create_moqt_client().await;

        (
            pubsub_relation_manager_wrapper,
            control_message_dispatcher,
            object_cache_storage_wrapper,
            start_forwarder_txes,
            client,
        )
    }

    async fn setup_upstream_subscription(
        pubsub_relation_manager_wrapper: PubSubRelationManagerWrapper,
        control_message_dispatcher: ControlMessageDispatcher,
        upstream_session_id: usize,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> u64 {
        let (message_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = control_message_dispatcher
            .get_tx()
            .send(ControlMessageDispatchCommand::Set {
                session_id: upstream_session_id,
                sender: message_tx,
            })
            .await;
        let _ = pubsub_relation_manager_wrapper
            .setup_publisher(10, upstream_session_id)
            .await;
        let _ = pubsub_relation_manager_wrapper
            .set_upstream_announced_namespace(track_namespace.clone(), upstream_session_id)
            .await;
        let (upstream_subscribe_id, _) = pubsub_relation_manager_wrapper
            .set_upstream_subscription(
                upstream_session_id,
                track_namespace.clone(),
                track_name.clone(),
                0,
                GroupOrder::Ascending,
                FilterType::LatestGroup,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        upstream_subscribe_id
    }

    async fn setup_downstream_subscription(
        pubsub_relation_manager_wrapper: PubSubRelationManagerWrapper,
        control_message_dispatcher: ControlMessageDispatcher,
        downstream_session_id: usize,
        downstream_subscribe_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
    ) {
        let (message_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = control_message_dispatcher
            .get_tx()
            .send(ControlMessageDispatchCommand::Set {
                session_id: downstream_session_id,
                sender: message_tx,
            })
            .await;
        let _ = pubsub_relation_manager_wrapper
            .setup_subscriber(10, downstream_session_id)
            .await;
        let _ = pubsub_relation_manager_wrapper
            .set_downstream_subscription(
                downstream_session_id,
                downstream_subscribe_id,
                0,
                track_namespace.clone(),
                track_name.clone(),
                0,
                GroupOrder::Ascending,
                FilterType::LatestGroup,
                None,
                None,
                None,
            )
            .await;
    }

    async fn setup_e2e_subscription(
        pubsub_relation_manager_wrapper: PubSubRelationManagerWrapper,
        control_message_dispatcher: ControlMessageDispatcher,
    ) -> (usize, u64, usize, u64) {
        let upstream_session_id = 0;
        let downstream_session_id = 10;
        let downstream_subscribe_id = 0;
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let track_name = "track_name".to_string();

        let upstream_subscribe_id = setup_upstream_subscription(
            pubsub_relation_manager_wrapper.clone(),
            control_message_dispatcher.clone(),
            upstream_session_id,
            track_namespace.clone(),
            track_name.clone(),
        )
        .await;
        setup_downstream_subscription(
            pubsub_relation_manager_wrapper.clone(),
            control_message_dispatcher.clone(),
            downstream_session_id,
            downstream_subscribe_id,
            track_namespace,
            track_name,
        )
        .await;

        pubsub_relation_manager_wrapper
            .set_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                downstream_session_id,
                downstream_subscribe_id,
            )
            .await
            .unwrap();

        (
            upstream_session_id,
            upstream_subscribe_id,
            downstream_session_id,
            downstream_subscribe_id,
        )
    }

    #[tokio::test]
    async fn unsubscribe_with_one_subscriber() {
        let subscribe_id = 0;
        let unsubscribe = Unsubscribe::new(subscribe_id);

        let (
            mut pubsub_relation_manager_wrapper,
            mut control_message_dispatcher,
            _object_cache_storage,
            _start_forwarder_txes,
            client,
        ) = initialize().await;
        let (
            upstream_session_id,
            upstream_subscribe_id,
            _downstream_session_id,
            _downstream_subscribe_id,
        ) = setup_e2e_subscription(
            pubsub_relation_manager_wrapper.clone(),
            control_message_dispatcher.clone(),
        )
        .await;

        let result = unsubscribe_handler(
            unsubscribe,
            &mut pubsub_relation_manager_wrapper,
            &mut control_message_dispatcher,
            &client,
        )
        .await;
        let downstream_subscribers = pubsub_relation_manager_wrapper
            .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
            .await
            .unwrap();

        assert!(result.is_ok());
        assert!(downstream_subscribers.is_empty());
    }

    #[tokio::test]
    async fn unsubscribe_with_two_subscribers() {
        let subscribe_id = 0;
        let unsubscribe = Unsubscribe::new(subscribe_id);

        let (
            mut pubsub_relation_manager_wrapper,
            mut control_message_dispatcher,
            _object_cache_storage,
            _start_forwarder_txes,
            client,
        ) = initialize().await;

        let (
            upstream_session_id,
            upstream_subscribe_id,
            _downstream_session_id,
            _downstream_subscribe_id,
        ) = setup_e2e_subscription(
            pubsub_relation_manager_wrapper.clone(),
            control_message_dispatcher.clone(),
        )
        .await;

        let second_downstream_session_id = 11;
        let second_downstream_subscribe_id = 0;

        setup_downstream_subscription(
            pubsub_relation_manager_wrapper.clone(),
            control_message_dispatcher.clone(),
            second_downstream_session_id,
            second_downstream_subscribe_id,
            Vec::from(["test".to_string(), "test".to_string()]),
            "track_name".to_string(),
        )
        .await;
        pubsub_relation_manager_wrapper
            .set_pubsub_relation(
                upstream_session_id,
                upstream_subscribe_id,
                second_downstream_session_id,
                second_downstream_subscribe_id,
            )
            .await
            .unwrap();

        let result = unsubscribe_handler(
            unsubscribe,
            &mut pubsub_relation_manager_wrapper,
            &mut control_message_dispatcher,
            &client,
        )
        .await;
        let downstream_subscribers = pubsub_relation_manager_wrapper
            .get_related_subscribers(upstream_session_id, upstream_subscribe_id)
            .await;

        assert!(result.is_ok());
        assert!(downstream_subscribers.is_ok());
        assert!(downstream_subscribers.unwrap().len() == 1);
    }
}

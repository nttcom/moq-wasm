pub(crate) mod handlers;
pub(crate) mod server_processes;

use crate::SenderToOpenSubscription;
use crate::constants::TerminationErrorCode;
use crate::modules::control_message_dispatcher::ControlMessageDispatcher;
use crate::modules::moqt_client::MOQTClient;
use crate::modules::{
    message_handlers::control_message::{
        handlers::unannounce_handler::unannounce_handler,
        server_processes::{
            announce_error_message::process_announce_error_message,
            announce_message::process_announce_message,
            announce_ok_message::process_announce_ok_message,
            client_setup_message::process_client_setup_message,
            subscribe_announces_message::process_subscribe_announces_message,
            subscribe_error_message::process_subscribe_error_message,
            subscribe_message::process_subscribe_message,
            subscribe_ok_message::process_subscribe_ok_message,
        },
    },
    moqt_client::MOQTClientStatus,
    object_cache_storage::wrapper::ObjectCacheStorageWrapper,
};
use anyhow::{Result, bail};
use bytes::{Buf, BytesMut};
use moqt_core::{
    constants::UnderlayType,
    control_message_type::ControlMessageType,
    messages::{control_messages::unannounce::UnAnnounce, moqt_payload::MOQTPayload},
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
    variable_integer::{read_variable_integer, write_variable_integer},
};
use server_processes::unsubscribe_message::process_unsubscribe_message;
use std::{collections::HashMap, io::Cursor, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum MessageProcessResult {
    Success(BytesMut),
    SuccessWithoutResponse,
    Failure(TerminationErrorCode, String),
    Fragment,
}

fn read_message_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<ControlMessageType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let message_type: ControlMessageType = match ControlMessageType::try_from(type_value) {
        Ok(v) => v,
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(message_type)
}

pub async fn control_message_handler(
    read_buf: &mut BytesMut,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    pubsub_relation_manager_repository: &mut dyn PubSubRelationManagerRepository,
    control_message_dispatcher: &mut ControlMessageDispatcher,
    object_cache_storage: &mut ObjectCacheStorageWrapper,
) -> MessageProcessResult {
    tracing::trace!("control_message_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::debug!("read_cur! {:?}", read_cur);

    // Read the message type
    let message_type = match read_message_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("message_type is wrong {:?}", err);
            return MessageProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received Message Type: {:?}", message_type);

    // Read the payload length
    let payload_length = read_variable_integer(&mut read_cur).unwrap();
    if payload_length == 0 {
        // The length is insufficient, so do nothing. Do not synchronize with the cursor.
        tracing::error!("fragmented {}", read_buf.len());
        return MessageProcessResult::Fragment;
    }

    read_buf.advance(read_cur.position() as usize);
    let mut payload_buf = read_buf.split_to(payload_length as usize);
    let mut write_buf = BytesMut::new();

    // Validate the timing of the message
    let is_invalid_timing_setup =
        message_type.is_setup_message() && client.status() != MOQTClientStatus::Connected;
    let is_invalid_timing_control =
        message_type.is_control_message() && client.status() != MOQTClientStatus::SetUp;

    if is_invalid_timing_control || is_invalid_timing_setup {
        let message = String::from("Invalid timing");
        tracing::error!(message);
        return MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, message);
    }

    let return_message_type = match message_type {
        ControlMessageType::ClientSetup => {
            match process_client_setup_message(
                &mut payload_buf,
                client,
                underlay_type,
                &mut write_buf,
                pubsub_relation_manager_repository,
            )
            .await
            {
                Ok(_) => ControlMessageType::ServerSetup,
                Err(err) => {
                    // TODO: To ensure future extensibility of MOQT, the peers MUST ignore unknown setup parameters.
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::Subscribe => {
            // TODO: Wait for subscribe_ok from the original publisher if the upstream subscription does not exist.
            match process_subscribe_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
                object_cache_storage,
                start_forwarder_txes,
            )
            .await
            {
                Ok(result) => match result {
                    Some(_) => ControlMessageType::SubscribeError,
                    None => {
                        return MessageProcessResult::SuccessWithoutResponse;
                    }
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::SubscribeOk => {
            // TODO: Merge to process_subscribe_message.
            match process_subscribe_ok_message(
                &mut payload_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
                client,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::SubscribeError => {
            match process_subscribe_error_message(
                &mut payload_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
                client,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::UnSubscribe => {
            match process_unsubscribe_message(
                &mut payload_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
                client,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::Announce => {
            match process_announce_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
            )
            .await
            {
                Ok(result) => match result {
                    Some(_) => ControlMessageType::AnnounceError,
                    None => {
                        return MessageProcessResult::SuccessWithoutResponse;
                    }
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::AnnounceOk => {
            match process_announce_ok_message(
                &mut payload_buf,
                client,
                pubsub_relation_manager_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::AnnounceError => {
            match process_announce_error_message(&mut payload_buf).await {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::SubscribeAnnounces => {
            match process_subscribe_announces_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                pubsub_relation_manager_repository,
                control_message_dispatcher,
            )
            .await
            {
                Ok(result) => match result {
                    Some(_) => ControlMessageType::SubscribeAnnouncesError,
                    None => {
                        return MessageProcessResult::SuccessWithoutResponse;
                    }
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::InternalError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::UnAnnounce => {
            let unannounce_message = UnAnnounce::depacketize(&mut payload_buf);

            if let Err(err) = unannounce_message {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::InternalError,
                    err.to_string(),
                );
            }

            // TODO: Not implemented yet
            let _unannounce_result = unannounce_handler(
                unannounce_message.unwrap(),
                client,
                pubsub_relation_manager_repository,
            )
            .await;

            return MessageProcessResult::Success(BytesMut::with_capacity(0));
        }
        ControlMessageType::GoAway => {
            todo!("GoAway");
        }
        unknown => {
            return MessageProcessResult::Failure(
                TerminationErrorCode::ProtocolViolation,
                format!("Unknown message type: {:?}", unknown),
            );
        }
    };

    tracing::info!("Return Message Type: {:?}", return_message_type.clone());
    let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);
    // Add type
    message_buf.extend(write_variable_integer(u8::from(return_message_type) as u64));
    // Add payload and payload length
    message_buf.extend(write_variable_integer(write_buf.len() as u64));
    message_buf.extend(write_buf);

    tracing::debug!("message_buf: {:#x?}", message_buf);

    MessageProcessResult::Success(message_buf)
}

#[cfg(test)]
pub(crate) mod test_helper_fn {
    use crate::SenderToOpenSubscription;
    use crate::modules::{
        control_message_dispatcher::{
            ControlMessageDispatchCommand, ControlMessageDispatcher, control_message_dispatcher,
        },
        message_handlers::control_message::{MessageProcessResult, control_message_handler},
        moqt_client::{MOQTClient, MOQTClientStatus},
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
    use bytes::BytesMut;
    use moqt_core::{constants::UnderlayType, variable_integer::write_variable_integer};
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::{Mutex, mpsc};

    pub async fn packetize_buf_and_execute_control_message_handler(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) -> MessageProcessResult {
        let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
        buf.extend(write_variable_integer(message_type_u8 as u64));
        buf.extend(write_variable_integer(bytes_array.len() as u64));
        buf.extend_from_slice(bytes_array);

        // Generate client
        let subscriber_sessin_id = 0;
        let senders_mock = senders::test_helper_fn::create_senders_mock();
        let mut client = MOQTClient::new(subscriber_sessin_id, senders_mock);
        client.update_status(client_status);

        // Generate PubSubRelationManagerWrapper
        let (track_namespace_tx, mut track_namespace_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut track_namespace_rx).await });
        let mut pubsub_relation_manager: PubSubRelationManagerWrapper =
            PubSubRelationManagerWrapper::new(track_namespace_tx);

        // Generate ControlMessageDispacher
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);

        tokio::spawn(
            async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
        );
        let mut control_message_dispatcher: ControlMessageDispatcher =
            ControlMessageDispatcher::new(control_message_dispatch_tx.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        // Prepare sender fot starting forwarder
        let start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Execute control_message_handler and get result

        control_message_handler(
            &mut buf,
            UnderlayType::WebTransport,
            &mut client,
            start_forwarder_txes,
            &mut pubsub_relation_manager,
            &mut control_message_dispatcher,
            &mut object_cache_storage,
        )
        .await
    }
}

#[cfg(test)]
mod success {
    use crate::modules::{
        message_handlers::control_message::{MessageProcessResult, test_helper_fn},
        moqt_client::MOQTClientStatus,
    };
    use moqt_core::control_message_type::ControlMessageType;

    #[tokio::test]
    async fn client_setup() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 10, // Supported Version(i): Value(0xff00000a) in 62bit
            1,  // Number of Parameters (i)
            0,  // SETUP Parameters (..): Type(Role)
            1,  // SETUP Parameters (..): Length
            2,  // SETUP Parameters (..): Role(Subscriber)
        ];
        let client_status = MOQTClientStatus::Connected;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(result, MessageProcessResult::Success(..)),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn subscribe() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 10, // Supported Version(i): Value(0xff00000a) in 62bit
            1,  // Number of Parameters (i)
            0,  // SETUP Parameters (..): Type(Role)
            1,  // SETUP Parameters (..): Length
            2,  // SETUP Parameters (..): Role(Subscriber)
        ];
        let client_status = MOQTClientStatus::Connected;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(result, MessageProcessResult::Success(..)),
            "result is not MessageProcessResult::Failure"
        );
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::{
        message_handlers::control_message::{MessageProcessResult, test_helper_fn},
        moqt_client::MOQTClientStatus,
    };
    use moqt_core::{constants::TerminationErrorCode, control_message_type::ControlMessageType};

    #[tokio::test]
    async fn client_setup_invalid_timing() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 10, // Supported Version(i): Value(0xff000a) in 62bit
            1,  // Number of Parameters (i)
            0,  // SETUP Parameters (..): Type(Role)
            1,  // SETUP Parameters (..): Length
            2,  // SETUP Parameters (..): Role(Subscriber)
        ];
        let wrong_client_status = MOQTClientStatus::SetUp; // Correct Status is Connected

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn subscribe_invalid_timing() {
        let message_type = ControlMessageType::Subscribe;
        let bytes_array = [
            15, // Track Namespace (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace (b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            0,   // StartGroup (Location): Location(None)
            0,   // StartObject (Location): Location(None)
            0,   // EndGroup (Location): Location(None)
            0,   // EndObject (Location): Location(None)
            1,   // Track Request Parameters (..): Number of Parameters
            0,   // Parameter Type (i)
            1,   // Parameter Length (i)
            0,   // Parameter Value (..): GroupSequence
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn subscribe_ok_invalid_timing() {
        let message_type = ControlMessageType::SubscribeOk;
        let bytes_array = [
            15, // Track Namespace (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace (b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
            1,   // Track ID (i)
            2,   // Expires (i)
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn unsubscribe_invalid_timing() {
        let message_type = ControlMessageType::UnSubscribe;
        let bytes_array = [
            15, // Track Namespace(b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace(b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn announce_invalid_timing() {
        let message_type = ControlMessageType::Announce;
        let bytes_array = [
            16, // Track Namespace(b): Length
            108, 105, 118, 101, 46, 101, 120, 97, 109, 112, 108, 101, 46, 99, 111,
            109, // Track Namespace(b): Value("live.example.com")
            1,   // Number of Parameters (i)
            2,   // Parameters (..): Parameter Type(AuthorizationInfo)
            4,   // Parameters (..): Length
            116, 101, 115, 116, // Parameters (..): Value("test")
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn client_setup_internal_error() {
        let message_type = ControlMessageType::ClientSetup;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::Connected;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::InternalError, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn subscribe_internal_error() {
        let message_type = ControlMessageType::Subscribe;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::InternalError, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn subscribe_ok_internal_error() {
        let message_type = ControlMessageType::SubscribeOk;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::InternalError, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    // #[tokio::test]
    // async fn unsubscribe_internal_error() {
    //     let message_type = ControlMessageType::UnSubscribe;
    //     let wrong_bytes_array = [0];
    //     let client_status = MOQTClientStatus::SetUp;

    //     assert_internal_error(message_type as u8, &wrong_bytes_array, client_status).await;
    // }

    #[tokio::test]
    async fn announce_internal_error() {
        let message_type = ControlMessageType::Announce;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::InternalError, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn unknown_message_type() {
        let message_type = 99;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
        )
        .await;

        assert!(
            matches!(
                result,
                MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
            ),
            "result is not MessageProcessResult::Failure"
        );
    }

    #[tokio::test]
    async fn fragment() {
        let message_type = ControlMessageType::Subscribe;
        let bytes_array = [];
        let client_status = MOQTClientStatus::SetUp;

        let result = test_helper_fn::packetize_buf_and_execute_control_message_handler(
            message_type as u8,
            &bytes_array,
            client_status,
        )
        .await;

        assert_eq!(result, MessageProcessResult::Fragment);
    }
}

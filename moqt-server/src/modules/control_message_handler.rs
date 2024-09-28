use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::modules::handlers::{
    announce_handler::AnnounceResponse, unannounce_handler::unannounce_handler,
};
use crate::modules::server_processes::{
    announce_message::process_announce_message, client_setup_message::process_client_setup_message,
    subscribe_message::process_subscribe_message,
    subscribe_ok_message::process_subscribe_ok_message,
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    constants::UnderlayType,
    control_message_type::ControlMessageType,
    messages::{moqt_payload::MOQTPayload, unannounce::UnAnnounce},
    moqt_client::MOQTClientStatus,
    variable_integer::{read_variable_integer, write_variable_integer},
    MOQTClient, SendStreamDispatcherRepository, TrackNamespaceManagerRepository,
};

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
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
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

    if read_cur.remaining() == 0 {
        // The length is insufficient, so do nothing. Do not synchronize with the cursor.
        tracing::error!("fragmented {}", read_buf.len());
        return MessageProcessResult::Fragment;
    }

    let payload_length = read_cur.remaining();
    read_buf.advance(read_cur.position() as usize);
    let mut payload_buf = read_buf.split_to(payload_length);
    let mut write_buf = BytesMut::new();

    let return_message_type = match message_type {
        ControlMessageType::ClientSetup => {
            if client.status() != MOQTClientStatus::Connected {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            match process_client_setup_message(
                &mut payload_buf,
                client,
                underlay_type,
                &mut write_buf,
            ) {
                Ok(_) => ControlMessageType::ServerSetup,
                Err(err) => {
                    // TODO: To ensure future extensibility of MOQT, the peers MUST ignore unknown setup parameters.
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::Subscribe => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            // TODO: Wait for subscribe_ok from the original publisher if the upstream subscription does not exist.
            match process_subscribe_message(
                &mut payload_buf,
                client,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        ControlMessageType::SubscribeOk => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            // TODO: Merge to process_subscribe_message.
            match process_subscribe_ok_message(
                &mut payload_buf,
                track_namespace_manager_repository,
                send_stream_dispatcher_repository,
            )
            .await
            {
                Ok(_) => {
                    return MessageProcessResult::SuccessWithoutResponse;
                }
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // ControlMessageType::SubscribeError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        ControlMessageType::UnSubscribe => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            let unsubscribe_message = UnAnnounce::depacketize(&mut payload_buf);

            if let Err(err) = unsubscribe_message {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: Not implemented yet
            let _unsubscribe_result = unannounce_handler(
                unsubscribe_message.unwrap(),
                client,
                track_namespace_manager_repository,
            );

            return MessageProcessResult::Success(BytesMut::with_capacity(0));
        }
        ControlMessageType::Announce => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            match process_announce_message(
                &mut payload_buf,
                client,
                &mut write_buf,
                track_namespace_manager_repository,
            )
            .await
            {
                Ok(announce_result) => match announce_result {
                    AnnounceResponse::Success(_) => ControlMessageType::AnnounceOk,
                    AnnounceResponse::Failure(_) => ControlMessageType::AnnounceError,
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // ControlMessageType::AnnounceOk => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        // ControlMessageType::AnnounceError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        ControlMessageType::UnAnnounce => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            let unannounce_message = UnAnnounce::depacketize(&mut payload_buf);

            if let Err(err) = unannounce_message {
                tracing::error!("{:#?}", err);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::GenericError,
                    err.to_string(),
                );
            }

            // TODO: Not implemented yet
            let _unannounce_result = unannounce_handler(
                unannounce_message.unwrap(),
                client,
                track_namespace_manager_repository,
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
    // Add payload
    message_buf.extend(write_buf);

    tracing::debug!("message_buf: {:#x?}", message_buf);

    MessageProcessResult::Success(message_buf)
}

#[cfg(test)]
pub(crate) mod test_fn {

    use crate::modules::control_message_handler::control_message_handler;
    use crate::modules::control_message_handler::MessageProcessResult;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use bytes::BytesMut;
    use moqt_core::constants::UnderlayType;
    use moqt_core::variable_integer::write_variable_integer;
    use moqt_core::{moqt_client::MOQTClientStatus, MOQTClient};
    use tokio::sync::mpsc;

    pub async fn packetize_buf_and_execute_control_message_handler(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) -> MessageProcessResult {
        let mut buf = BytesMut::with_capacity(bytes_array.len() + 8);
        buf.extend(write_variable_integer(message_type_u8 as u64));
        buf.extend_from_slice(bytes_array);

        // Generate client
        let subscriber_sessin_id = 0;
        let mut client = MOQTClient::new(subscriber_sessin_id);
        client.update_status(client_status);

        // Generate TrackNamespaceManager
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });
        let mut track_namespace_manager: TrackNamespaceManager =
            TrackNamespaceManager::new(track_namespace_tx);

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        // Execute control_message_handler and get result
        control_message_handler(
            &mut buf,
            UnderlayType::WebTransport,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await
    }
}

#[cfg(test)]
mod success {
    use crate::modules::control_message_handler::MessageProcessResult;
    use moqt_core::control_message_type::ControlMessageType;
    use moqt_core::moqt_client::MOQTClientStatus;

    use crate::modules::control_message_handler::test_fn;

    async fn assert_success(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) {
        let result = test_fn::packetize_buf_and_execute_control_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
        )
        .await;

        // Check if MessageProcessResult::Failure(TerminationErrorCode::GenericError, _)
        if let MessageProcessResult::Success(..) = result {
        } else {
            panic!("result is not MessageProcessResult::Succsess");
        };
    }

    #[tokio::test]
    async fn client_setup_succsess() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 1, // Supported Version(i): Value(0xff000001) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            2, // SETUP Parameters (..): Role(Delivery)
        ];
        let client_status = MOQTClientStatus::Connected;

        assert_success(message_type as u8, &bytes_array, client_status).await;
    }

    #[tokio::test]
    async fn subscribe_succsess() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 1, // Supported Version(i): Value(0xff000001) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            2, // SETUP Parameters (..): Role(Delivery)
        ];
        let client_status = MOQTClientStatus::Connected;

        assert_success(message_type as u8, &bytes_array, client_status).await;
    }
}

#[cfg(test)]
mod failure {
    use crate::constants::TerminationErrorCode;
    use crate::modules::control_message_handler::MessageProcessResult;
    use moqt_core::control_message_type::ControlMessageType;
    use moqt_core::moqt_client::MOQTClientStatus;

    use crate::modules::control_message_handler::test_fn;

    async fn assert_protocol_violation(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) {
        let result = test_fn::packetize_buf_and_execute_control_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
        )
        .await;

        // Check if MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, _)
        if let MessageProcessResult::Failure(code, _) = result {
            assert_eq!(code, TerminationErrorCode::ProtocolViolation);
        } else {
            panic!("result is not MessageProcessResult::Failure");
        };
    }

    async fn assert_generic_error(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) {
        let result = test_fn::packetize_buf_and_execute_control_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
        )
        .await;

        // Check if MessageProcessResult::Failure(TerminationErrorCode::GenericError, _)
        if let MessageProcessResult::Failure(code, _) = result {
            assert_eq!(code, TerminationErrorCode::GenericError);
        } else {
            panic!("result is not MessageProcessResult::Failure");
        };
    }

    async fn assert_fragment(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
    ) {
        let result = test_fn::packetize_buf_and_execute_control_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
        )
        .await;

        assert_eq!(result, MessageProcessResult::Fragment);
    }

    #[tokio::test]
    async fn client_setup_invalid_timing() {
        let message_type = ControlMessageType::ClientSetup;
        let bytes_array = [
            1,   // Number of Supported Versions (i)
            192, // Supported Version (i): Length(11 of 2MSB)
            0, 0, 0, 255, 0, 0, 1, // Supported Version(i): Value(0xff000001) in 62bit
            1, // Number of Parameters (i)
            0, // SETUP Parameters (..): Type(Role)
            1, // SETUP Parameters (..): Length
            2, // SETUP Parameters (..): Role(Delivery)
        ];
        let wrong_client_status = MOQTClientStatus::SetUp; // Correct Status is Connected

        assert_protocol_violation(message_type as u8, &bytes_array, wrong_client_status).await;
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

        assert_protocol_violation(message_type as u8, &bytes_array, wrong_client_status).await;
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

        assert_protocol_violation(message_type as u8, &bytes_array, wrong_client_status).await;
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

        assert_protocol_violation(message_type as u8, &bytes_array, wrong_client_status).await;
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

        assert_protocol_violation(message_type as u8, &bytes_array, wrong_client_status).await;
    }

    #[tokio::test]
    async fn client_setup_generic_error() {
        let message_type = ControlMessageType::ClientSetup;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::Connected;

        assert_generic_error(message_type as u8, &wrong_bytes_array, client_status).await;
    }

    #[tokio::test]
    async fn subscribe_generic_error() {
        let message_type = ControlMessageType::Subscribe;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        assert_generic_error(message_type as u8, &wrong_bytes_array, client_status).await;
    }

    #[tokio::test]
    async fn subscribe_ok_generic_error() {
        let message_type = ControlMessageType::SubscribeOk;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        assert_generic_error(message_type as u8, &wrong_bytes_array, client_status).await;
    }

    // #[tokio::test]
    // async fn unsubscribe_generic_error() {
    //     let message_type = ControlMessageType::UnSubscribe;
    //     let wrong_bytes_array = [0];
    //     let client_status = MOQTClientStatus::SetUp;

    //     assert_generic_error(message_type as u8, &wrong_bytes_array, client_status).await;
    // }

    #[tokio::test]
    async fn announce_generic_error() {
        let message_type = ControlMessageType::Announce;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        assert_generic_error(message_type as u8, &wrong_bytes_array, client_status).await;
    }

    #[tokio::test]
    async fn unknown_message_type() {
        let message_type = 99;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;

        assert_protocol_violation(message_type, &wrong_bytes_array, client_status).await;
    }

    #[tokio::test]
    async fn fragment() {
        let message_type = ControlMessageType::Subscribe;
        let bytes_array = [];
        let client_status = MOQTClientStatus::SetUp;

        assert_fragment(message_type as u8, &bytes_array, client_status).await;
    }
}

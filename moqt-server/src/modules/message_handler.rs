use std::io::Cursor;

use crate::constants::TerminationErrorCode;
use crate::modules::handlers::{
    announce_handler::AnnounceResponse, unannounce_handler::unannounce_handler,
};
use crate::modules::server_processes::{
    announce_message::process_announce_message,
    client_setup_message::process_client_setup_message,
    object_message::{process_object_with_payload_length, process_object_without_payload_length},
    subscribe_message::process_subscribe_message,
    subscribe_ok_message::process_subscribe_ok_message,
};
use anyhow::{bail, Result};
use bytes::{Buf, BytesMut};
use moqt_core::{
    constants::UnderlayType,
    message_type::MessageType,
    messages::{moqt_payload::MOQTPayload, unannounce::UnAnnounce},
    moqt_client::MOQTClientStatus,
    stream_type::StreamType,
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

fn read_message_type(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<MessageType> {
    let type_value = match read_variable_integer(read_cur) {
        Ok(v) => v as u8,
        Err(err) => {
            bail!(err.to_string());
        }
    };

    let message_type: MessageType = match MessageType::try_from(type_value) {
        Ok(v) => v,
        Err(err) => {
            bail!(err.to_string());
        }
    };
    Ok(message_type)
}

pub async fn message_handler(
    read_buf: &mut BytesMut,
    stream_type: StreamType,
    underlay_type: UnderlayType,
    client: &mut MOQTClient,
    track_namespace_manager_repository: &mut dyn TrackNamespaceManagerRepository,
    send_stream_dispatcher_repository: &mut dyn SendStreamDispatcherRepository,
) -> MessageProcessResult {
    tracing::trace!("message_handler! {}", read_buf.len());

    let mut read_cur = Cursor::new(&read_buf[..]);
    tracing::debug!("read_cur! {:?}", read_cur);

    // Read the message type
    let message_type = match read_message_type(&mut read_cur) {
        Ok(v) => v,
        Err(err) => {
            read_buf.advance(read_cur.position() as usize);

            tracing::error!("message_type is wrong {:?}", err);
            return MessageProcessResult::Failure(
                TerminationErrorCode::GenericError,
                err.to_string(),
            );
        }
    };
    tracing::info!("Received Message Type: {:?}", message_type);

    if message_type.is_object_message() {
        // Object message must be sent on unidirectional stream
        if stream_type == StreamType::Bi {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Object message must be sent on unidirectional stream");
            tracing::error!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, message);
        }
    } else {
        // Control message must be sent on bidirectional stream
        if stream_type == StreamType::Uni {
            read_buf.advance(read_cur.position() as usize);

            let message = String::from("Control message must be sent on bidirectional stream");
            tracing::error!(message);
            return MessageProcessResult::Failure(TerminationErrorCode::ProtocolViolation, message);
        }
    }

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
        MessageType::ObjectWithPayloadLength => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            match process_object_with_payload_length(
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
        MessageType::ObjectWithoutPayloadLength => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

            match process_object_without_payload_length(
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
        MessageType::ClientSetup => {
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
                Ok(_) => MessageType::ServerSetup,
                Err(err) => {
                    // TODO: To ensure future extensibility of MOQT, the peers MUST ignore unknown setup parameters.
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        MessageType::Subscribe => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

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
        MessageType::SubscribeOk => {
            if client.status() != MOQTClientStatus::SetUp {
                let message = String::from("Invalid timing");
                tracing::error!(message);
                return MessageProcessResult::Failure(
                    TerminationErrorCode::ProtocolViolation,
                    message,
                );
            }

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
        // MessageType::SubscribeError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnSubscribe => {
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
        MessageType::Announce => {
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
                    AnnounceResponse::Success(_) => MessageType::AnnounceOk,
                    AnnounceResponse::Failure(_) => MessageType::AnnounceError,
                },
                Err(err) => {
                    return MessageProcessResult::Failure(
                        TerminationErrorCode::GenericError,
                        err.to_string(),
                    );
                }
            }
        }
        // MessageType::AnnounceOk => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        // MessageType::AnnounceError => {
        //     if client.status() != MOQTClientStatus::SetUp {
        //         return MessageProcessResult::Failure;
        //     }
        // }
        MessageType::UnAnnounce => {
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
        MessageType::GoAway => {
            todo!("GoAway");
        }
        unknown => {
            return MessageProcessResult::Failure(
                TerminationErrorCode::GenericError,
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

    use crate::modules::message_handler::message_handler;
    use crate::modules::message_handler::MessageProcessResult;
    use crate::modules::send_stream_dispatcher::{
        send_stream_dispatcher, SendStreamDispatchCommand, SendStreamDispatcher,
    };
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackCommand, TrackNamespaceManager,
    };
    use bytes::BytesMut;
    use moqt_core::constants::UnderlayType;
    use moqt_core::messages::moqt_payload::MOQTPayload;
    use moqt_core::stream_type::StreamType;
    use moqt_core::variable_integer::write_variable_integer;
    use moqt_core::TrackNamespaceManagerRepository;
    use moqt_core::{moqt_client::MOQTClientStatus, MOQTClient};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    pub async fn packetize_buf_and_execute_message_handler(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
        stream_type: StreamType,
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

        // Execute message_handler and get result
        message_handler(
            &mut buf,
            stream_type,
            UnderlayType::WebTransport,
            &mut client,
            &mut track_namespace_manager,
            &mut send_stream_dispatcher,
        )
        .await
    }

    pub async fn packetize_buf_and_execute_message_handler_with_subscriber(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
        stream_type: StreamType,
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

        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";
        let track_id = 0;

        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        // Generate SendStreamDispacher
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);

        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let mut send_stream_dispatcher: SendStreamDispatcher =
            SendStreamDispatcher::new(send_stream_tx.clone());

        let (uni_relay_tx, _) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
        let _ = send_stream_tx
            .send(SendStreamDispatchCommand::Set {
                session_id: subscriber_session_id,
                stream_type: "unidirectional_stream".to_string(),
                sender: uni_relay_tx,
            })
            .await;

        // Execute message_handler and get result
        message_handler(
            &mut buf,
            stream_type,
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
    use crate::modules::message_handler::MessageProcessResult;
    use moqt_core::message_type::MessageType;
    use moqt_core::moqt_client::MOQTClientStatus;
    use moqt_core::stream_type::StreamType;

    use crate::modules::message_handler::test_fn;

    async fn assert_success(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
        stream_type: StreamType,
    ) {
        let result = test_fn::packetize_buf_and_execute_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
            stream_type,
        )
        .await;

        // Check if MessageProcessResult::Failure(TerminationErrorCode::GenericError, _)
        if let MessageProcessResult::Success(..) = result {
        } else {
            panic!("result is not MessageProcessResult::Succsess");
        };
    }

    async fn assert_success_without_response(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
        stream_type: StreamType,
    ) {
        let result = test_fn::packetize_buf_and_execute_message_handler_with_subscriber(
            message_type_u8,
            bytes_array,
            client_status,
            stream_type,
        )
        .await;

        assert_eq!(result, MessageProcessResult::SuccessWithoutResponse);
    }

    #[tokio::test]
    async fn client_setup_succsess() {
        let message_type = MessageType::ClientSetup;
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
        let stream_type = StreamType::Bi;

        assert_success(message_type as u8, &bytes_array, client_status, stream_type).await;
    }

    #[tokio::test]
    async fn object_with_payload_length_success_without_response() {
        let message_type = MessageType::ObjectWithPayloadLength;
        let bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload Length (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Uni;

        assert_success_without_response(
            message_type as u8,
            &bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }
}

#[cfg(test)]
mod failure {
    use crate::constants::TerminationErrorCode;
    use crate::modules::message_handler::MessageProcessResult;
    use moqt_core::message_type::MessageType;
    use moqt_core::moqt_client::MOQTClientStatus;
    use moqt_core::stream_type::StreamType;

    use crate::modules::message_handler::test_fn;

    async fn assert_protocol_violation(
        message_type_u8: u8,
        bytes_array: &[u8],
        client_status: MOQTClientStatus,
        stream_type: StreamType,
    ) {
        let result = test_fn::packetize_buf_and_execute_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
            stream_type,
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
        stream_type: StreamType,
    ) {
        let result = test_fn::packetize_buf_and_execute_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
            stream_type,
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
        stream_type: StreamType,
    ) {
        let result = test_fn::packetize_buf_and_execute_message_handler(
            message_type_u8,
            bytes_array,
            client_status,
            stream_type,
        )
        .await;

        assert_eq!(result, MessageProcessResult::Fragment);
    }

    #[tokio::test]
    async fn object_on_bidirectional_stream() {
        let message_type = MessageType::ObjectWithPayloadLength;
        let bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload Length (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let client_status = MOQTClientStatus::SetUp;
        let wrong_stream_type = StreamType::Bi; // Correct direction is Uni

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            client_status,
            wrong_stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn control_on_unidirectional_stream() {
        let message_type = MessageType::ClientSetup;
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
        let wrong_stream_type = StreamType::Uni; // Correct direction is Bi

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            client_status,
            wrong_stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn object_with_payload_length_invalid_timing() {
        let message_type = MessageType::ObjectWithPayloadLength;
        let bytes_array = [
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload Length (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp
        let stream_type = StreamType::Uni;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn object_without_payload_length_invalid_timing() {
        let message_type = MessageType::ObjectWithoutPayloadLength;
        let bytes_array = [
            2, // Message Type: Object without payload length
            0, // Track ID (i)
            1, // Group Sequence (i)
            2, // Object Sequence (i)
            3, // Object Send Order (i)
            3, // Object Payload (b): Length
            0, 1, 2, // Object Payload (b): Value
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp
        let stream_type = StreamType::Uni;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn client_setup_invalid_timing() {
        let message_type = MessageType::ClientSetup;
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
        let stream_type = StreamType::Bi;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn subscribe_invalid_timing() {
        let message_type = MessageType::Subscribe;
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
        let stream_type = StreamType::Bi;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn subscribe_ok_invalid_timing() {
        let message_type = MessageType::SubscribeOk;
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
        let stream_type = StreamType::Bi;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn unsubscribe_invalid_timing() {
        let message_type = MessageType::UnSubscribe;
        let bytes_array = [
            15, // Track Namespace(b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109, 101, 115, 112, 97, 99,
            101, // Track Namespace(b): Value("track_namespace")
            10,  // Track Name (b): Length
            116, 114, 97, 99, 107, 95, 110, 97, 109,
            101, // Track Name (b): Value("track_name")
        ];
        let wrong_client_status = MOQTClientStatus::Connected; // Correct Status is SetUp
        let stream_type = StreamType::Bi;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn announce_invalid_timing() {
        let message_type = MessageType::Announce;
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
        let stream_type = StreamType::Bi;

        assert_protocol_violation(
            message_type as u8,
            &bytes_array,
            wrong_client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn object_with_payload_length_generic_error() {
        let message_type = MessageType::ObjectWithPayloadLength;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Uni;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn object_without_payload_length_generic_error() {
        let message_type = MessageType::ObjectWithoutPayloadLength;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Uni;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn client_setup_generic_error() {
        let message_type = MessageType::ClientSetup;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::Connected;
        let stream_type = StreamType::Bi;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn subscribe_generic_error() {
        let message_type = MessageType::Subscribe;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Bi;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn subscribe_ok_generic_error() {
        let message_type = MessageType::SubscribeOk;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Bi;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    // #[tokio::test]
    // async fn unsubscribe_generic_error() {
    //     let message_type = MessageType::UnSubscribe;
    //     let wrong_bytes_array = [0];
    //     let client_status = MOQTClientStatus::SetUp;
    //     let stream_type = StreamType::Bi;

    //     assert_generic_error(message_type as u8, &wrong_bytes_array, client_status, stream_type).await;
    // }

    #[tokio::test]
    async fn announce_generic_error() {
        let message_type = MessageType::Announce;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Bi;

        assert_generic_error(
            message_type as u8,
            &wrong_bytes_array,
            client_status,
            stream_type,
        )
        .await;
    }

    #[tokio::test]
    async fn unknown_message_type() {
        let message_type = 99;
        let wrong_bytes_array = [0];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Bi;

        assert_generic_error(message_type, &wrong_bytes_array, client_status, stream_type).await;
    }

    #[tokio::test]
    async fn fragment() {
        let message_type = MessageType::Subscribe;
        let bytes_array = [];
        let client_status = MOQTClientStatus::SetUp;
        let stream_type = StreamType::Bi;

        assert_fragment(message_type as u8, &bytes_array, client_status, stream_type).await;
    }
}

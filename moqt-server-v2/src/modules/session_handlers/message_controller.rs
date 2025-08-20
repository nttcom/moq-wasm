use std::{collections::HashMap, io::Cursor, result, sync::Arc};

use anyhow::bail;
use bytes::{Buf, BytesMut};

use crate::modules::session_handlers::{
    bi_stream::BiStreamTrait,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::setup_message_handler::SetupMessageHandler,
        message_process_result::MessageProcessResult, variable_integer::read_variable_integer,
    },
};

pub(crate) struct MessageController {
    join_handlers: std::sync::Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>,
}

impl Drop for MessageController {
    fn drop(&mut self) {
        self.join_handlers
            .lock()
            .unwrap()
            .iter()
            .for_each(|(_, j)| j.abort());
    }
}

impl MessageController {
    pub fn new() -> Arc<Self> {
        let _self = Self {
            join_handlers: std::sync::Mutex::new(HashMap::new()),
        };
        Arc::new(_self)
    }

    pub async fn handle_recv_message(
        self: Arc<Self>,
        stream: &Box<dyn BiStreamTrait>,
        mut read_buffer: BytesMut,
    ) -> anyhow::Result<()> {
        tracing::trace!("control_message_handler! {}", read_buffer.len());

        let mut read_cur = Cursor::new(&read_buffer[..]);
        tracing::debug!("read_cur! {:?}", read_cur);

        let message_type = match self.clone().read_message_type(&mut read_cur) {
            Ok(v) => v,
            Err(err) => {
                read_buffer.advance(read_cur.position() as usize);

                tracing::error!("message_type is wrong {:?}", err);
                return Err(err);
            }
        };
        tracing::info!("Received Message Type: {:?}", message_type);
        let payload_length = read_variable_integer(&mut read_cur).unwrap();
        if payload_length == 0 {
            // The length is insufficient, so do nothing. Do not synchronize with the cursor.
            tracing::error!("fragmented {}", read_buffer.len());
            bail!("payload length is 0.")
        }

        read_buffer.advance(read_cur.position() as usize);
        let mut payload_buf = read_buffer.split_to(payload_length as usize);

        self.handle_control_message(stream, message_type, &mut payload_buf)
            .await
    }

    fn read_message_type(
        self: Arc<Self>,
        read_cur: &mut std::io::Cursor<&[u8]>,
    ) -> anyhow::Result<ControlMessageType> {
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

    async fn handle_control_message(
        self: Arc<Self>,
        stream: &Box<dyn BiStreamTrait>,
        message_type: ControlMessageType,
        payload_buffer: &mut BytesMut,
    ) -> anyhow::Result<()> {
        let message = match message_type {
            ControlMessageType::ClientSetup => {
                SetupMessageHandler::create_server_setup(payload_buffer)
            }
            ControlMessageType::ServerSetup => {
                SetupMessageHandler::handle_server_setup(payload_buffer)
            }
            others => panic!("{}", format!("unsupported on the server. {:?}", others)),
        };
        match message {
            MessageProcessResult::Success(buffer) => self.response(stream, &buffer).await,
            MessageProcessResult::SuccessWithoutResponse => Ok(()),
            MessageProcessResult::Failure(code, message) => {
                bail!(format!("failed... code: {:?}, message: {}", code, message))
            }
            MessageProcessResult::Fragment => bail!("failed to send full message"),
        }
    }

    async fn response(self: Arc<Self>, stream: &Box<dyn BiStreamTrait>, buffer: &BytesMut) -> anyhow::Result<()> {
        let result = stream.send(buffer).await;
        Ok(())
    }

    fn start_receiver(self: Arc<Self>, stream: Box<dyn BiStreamTrait>) {
        let stream_id = stream.get_stream_id();
        let join_handle = self.clone().create_join_handle(stream);
        self.join_handlers
            .lock()
            .unwrap()
            .insert(stream_id, join_handle);
    }

    fn create_join_handle(
        self: Arc<Self>,
        mut stream: Box<dyn BiStreamTrait>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::Builder::new()
            .name("Messsage Receiver")
            .spawn(async move {
                loop {
                    let _self = self.clone();
                    let buffer = stream.receive().await;
                    if let Err(e) = buffer {
                        tracing::error!("failed to receive message: {}", e.to_string());
                        break;
                    }
                    match _self.handle_recv_message(&stream, buffer.unwrap()).await {
                        Ok(_) => tracing::info!("Handling received message has succeeded."),
                        Err(_) => todo!(),
                    }
                }
            })
            .unwrap()
    }
}

#[cfg(test)]
mod message_controller_test {
    use crate::modules::session_handlers::{
        bi_stream::MockBiStreamTrait,
        message_controller::MessageController,
        messages::{
            control_message_type::ControlMessageType, control_messages::client_setup::ClientSetup,
            moqt_payload::MOQTPayload, variable_integer::write_variable_integer,
        },
    };
    use bytes::BytesMut;

    #[tokio::test]
    async fn handle_recv_message_success() {
        // setup
        let mut mock_bi_stream = MockBiStreamTrait::new();
        mock_bi_stream.expect_send().returning(|_| Ok(()));
        let server_message_controller = MessageController::new();

        let client_setup = ClientSetup::new(vec![1], vec![]);
        let mut client_setup_payload = BytesMut::new();
        client_setup.packetize(&mut client_setup_payload);

        let mut message = BytesMut::new();
        message.extend_from_slice(&write_variable_integer(
            ControlMessageType::ClientSetup as u64,
        ));
        message.extend_from_slice(&write_variable_integer(client_setup_payload.len() as u64));
        message.extend_from_slice(&client_setup_payload);

        // execution
        let result = server_message_controller
            .handle_recv_message(&mut message)
            .await;

        // verification
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn handle_recv_message_payload_length_is_0() {
        // setup
        let mut mock_bi_stream = Box::new(MockBiStreamTrait::new());
        mock_bi_stream.expect_send().returning(|_| Ok(()));
        let server_message_controller = MessageController::new();

        let mut message = BytesMut::new();
        message.extend_from_slice(&write_variable_integer(
            ControlMessageType::ClientSetup as u64,
        ));
        message.extend_from_slice(&write_variable_integer(0));

        // execution
        let result = server_message_controller
            .handle_recv_message(&mut message)
            .await;

        // verification
        assert!(result.is_err());
    }

    #[tokio::test]
    #[should_panic]
    async fn handle_recv_message_payload_length_mismatch() {
        // setup
        let mut mock_bi_stream = MockBiStreamTrait::new();
        mock_bi_stream.expect_send().returning(|_| Ok(()));
        let server_message_controller = MessageController::new();

        let client_setup = ClientSetup::new(vec![1], vec![]);
        let mut client_setup_payload = BytesMut::new();
        client_setup.packetize(&mut client_setup_payload);

        let mut message = BytesMut::new();
        message.extend_from_slice(&write_variable_integer(
            ControlMessageType::ClientSetup as u64,
        ));
        message.extend_from_slice(&write_variable_integer(10000));
        message.extend_from_slice(&client_setup_payload);

        // execution
        let _ = server_message_controller
            .handle_recv_message(&mut message)
            .await;
    }

    #[tokio::test]
    async fn handle_recv_message_invalid_payload() {
        // setup
        let mut mock_bi_stream = MockBiStreamTrait::new();
        mock_bi_stream.expect_send().returning(|_| Ok(()));
        let server_message_controller = MessageController::new();

        let invalid_payload = BytesMut::from(&b"invalid payload"[..]);

        let mut message = BytesMut::new();
        message.extend_from_slice(&write_variable_integer(
            ControlMessageType::ClientSetup as u64,
        ));
        message.extend_from_slice(&write_variable_integer(invalid_payload.len() as u64));
        message.extend_from_slice(&invalid_payload);

        // execution
        let result = server_message_controller
            .handle_recv_message(&mut message)
            .await;

        // verification
        assert!(result.is_err());
    }
}

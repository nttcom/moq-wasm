use std::{io::Cursor, result};

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

struct MessageController {
    bi_stream: Box<dyn BiStreamTrait>,
}

impl MessageController {
    pub fn new(bi_stream: Box<dyn BiStreamTrait>) -> Self {
        Self { bi_stream }
    }

    pub async fn handle_recv_message(&self, read_buffer: &mut BytesMut) -> anyhow::Result<()> {
        tracing::trace!("control_message_handler! {}", read_buffer.len());

        let mut read_cur = Cursor::new(&read_buffer[..]);
        tracing::debug!("read_cur! {:?}", read_cur);

        let message_type = match self.read_message_type(&mut read_cur) {
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

        self.handle_control_message(message_type, &mut payload_buf)
            .await
    }

    fn read_message_type(
        &self,
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
        &self,
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
            MessageProcessResult::Success(buffer) => self.response(&buffer).await,
            MessageProcessResult::SuccessWithoutResponse => Ok(()),
            MessageProcessResult::Failure(code, message) => {
                bail!(format!("failed... code: {:?}, message: {}", code, message))
            }
            MessageProcessResult::Fragment => bail!("failed to send full message"),
        }
    }

    async fn response(&self, buffer: &BytesMut) -> anyhow::Result<()> {
        self.bi_stream.send(buffer).await
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
        let server_message_controller = MessageController::new(Box::new(mock_bi_stream));

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
        let mut mock_bi_stream = MockBiStreamTrait::new();
        mock_bi_stream.expect_send().returning(|_| Ok(()));
        let server_message_controller = MessageController::new(Box::new(mock_bi_stream));

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
        let server_message_controller = MessageController::new(Box::new(mock_bi_stream));

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
        let server_message_controller = MessageController::new(Box::new(mock_bi_stream));

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

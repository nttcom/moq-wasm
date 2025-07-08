use std::{io::Cursor, result};

use anyhow::bail;
use bytes::{Buf, BytesMut};

use crate::modules::session_handlers::{
    bi_stream::BiStreamTrait,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::setup_message_builder::SetupMessageBuilder,
        message_process_result::MessageProcessResult, variable_integer::read_variable_integer,
    },
};

struct ServerMessageController {
    bi_stream: Box<dyn BiStreamTrait>,
}

impl ServerMessageController {
    pub fn new(bi_stream: Box<dyn BiStreamTrait>) -> Self {
        Self { bi_stream }
    }

    pub fn handle_recv_message(&self, read_buffer: &mut BytesMut) -> anyhow::Result<()> {
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

        Self::handle_control_message(&self, message_type, &mut payload_buf)
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

    fn handle_control_message(
        &self,
        message_type: ControlMessageType,
        payload_buffer: &mut BytesMut,
    ) -> anyhow::Result<()> {
        let message = match message_type {
            ControlMessageType::ClientSetup => {
                SetupMessageBuilder::create_server_setup(payload_buffer)
            }
            others => panic!("{}", format!("unsupported on the server. {:?}", others)),
        };
        match message {
            MessageProcessResult::Success(buffer) => self.response(&buffer),
            MessageProcessResult::SuccessWithoutResponse => Ok(()),
            MessageProcessResult::Failure(code, message) => bail!(""),
            MessageProcessResult::Fragment => todo!(),
        }
    }

    fn response(&self, buffer: &BytesMut) {
        self.bi_stream.send(buffer);
    }
}

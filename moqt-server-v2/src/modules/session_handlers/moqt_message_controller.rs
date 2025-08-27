use std::{io::Cursor, sync::Arc};

use anyhow::bail;
use bytes::{Buf, BytesMut};

use crate::modules::session_handlers::{
    constants,
    messages::{
        control_message_type::ControlMessageType, control_messages::client_setup::ClientSetup, message_process_result::MessageProcessResult, moqt_payload::MOQTPayload, variable_integer::read_variable_integer
    },
    moqt_bi_stream::MOQTBiStream,
    moqt_message_builder::MOQTMessageBuilder,
};

pub(crate) struct MOQTMessageController;

impl MOQTMessageController {
    pub(crate) fn new() -> Self {
        Self
    }

    pub fn handle(&self, mut read_buffer: BytesMut) -> anyhow::Result<MessageProcessResult> {
        let mut read_cursor = Cursor::new(&read_buffer[..]);
        tracing::debug!("read_cur! {:?}", read_cursor);
        let message_type = self.read_message_type(&mut read_cursor)?;
        tracing::info!("Received Message Type: {:?}", message_type);
        let payload_length = read_variable_integer(&mut read_cursor).unwrap();
        if payload_length == 0 {
            // The length is insufficient, so do nothing. Do not synchronize with the cursor.
            tracing::error!("fragmented {}", read_buffer.len());
            bail!("payload length is 0.")
        }

        read_buffer.advance(read_cursor.position() as usize);

        let mut payload = read_buffer.split_to(payload_length as usize);
        Ok(self.handle_control_message(message_type, &mut payload))
    }

    fn read_message_type(
        &self,
        read_cur: &mut std::io::Cursor<&[u8]>,
    ) -> anyhow::Result<ControlMessageType> {
        let type_value = match read_variable_integer(read_cur) {
            Ok(v) => v as u8,
            Err(e) => {
                tracing::error!("message_type is wrong {:?}", e);
                bail!(e.to_string());
            }
        };

        let message_type: ControlMessageType = match ControlMessageType::try_from(type_value) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("message_type is wrong {:?}", e);
                bail!(e.to_string());
            }
        };
        Ok(message_type)
    }

    fn handle_control_message(
        &self,
        message_type: ControlMessageType,
        payload_buffer: &mut BytesMut,
    ) -> anyhow::Result<dyn MOQTPayload> {
        match message_type {
            ControlMessageType::ClientSetup => {
                ClientSetup::depacketize(payload_buffer)
            }
            ControlMessageType::ServerSetup => {
                
            }
            others => panic!("{}", format!("unsupported on the server. {:?}", others)),
        }
    }
}

use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            messages::object::{
                datagram_object::DatagramObject,
                extension_header::ExtensionHeaderType,
                key_value_pair::{KeyValuePair, VariantType},
            },
            sessions::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct DatagramSender<T: TransportProtocol> {
    pub track_alias: u64,
    pub end_of_group: bool,
    session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramSender<T> {
    pub fn new(track_alias: u64, session_context: Arc<SessionContext<T>>) -> Self {
        Self {
            track_alias,
            end_of_group: false,
            session_context,
        }
    }

    pub fn create_datagram_object(
        &self,
        group_id: u64,
        object_id: Option<u64>,
        publisher_priority: u8,
        prior_object_id_gap: Option<u64>,
        prior_group_id_gap: Option<u64>,
        immutable_extensions: Vec<u8>,
        bytes: BytesMut,
    ) -> anyhow::Result<DatagramObject> {
        let (object_id, has_object_id) = if object_id.is_none() {
            (0, false)
        } else {
            (object_id.unwrap(), true)
        };
        let has_extension_headers = prior_group_id_gap.is_some()
            || prior_object_id_gap.is_some()
            || !immutable_extensions.is_empty();
        let extension_headers = if has_extension_headers {
            Self::create_extension_headers(
                prior_group_id_gap,
                prior_object_id_gap,
                immutable_extensions,
            )
        } else {
            vec![]
        };

        let message_type = self.fix_message_type(has_extension_headers, has_object_id, true)?;
        Ok(DatagramObject {
            message_type,
            track_alias: self.track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_status: None,
            object_payload: bytes.to_vec(),
        })
    }

    pub fn create_datagram_object_with_extension_headers(
        &self,
        group_id: u64,
        object_id: Option<u64>,
        publisher_priority: u8,
        extension_headers: Vec<KeyValuePair>,
        bytes: BytesMut,
    ) -> anyhow::Result<DatagramObject> {
        let (object_id, has_object_id) = if let Some(object_id) = object_id {
            (object_id, true)
        } else {
            (0, false)
        };

        let message_type =
            self.fix_message_type(!extension_headers.is_empty(), has_object_id, true)?;
        Ok(DatagramObject {
            message_type,
            track_alias: self.track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_status: None,
            object_payload: bytes.to_vec(),
        })
    }

    fn fix_message_type(
        &self,
        has_extension_headers: bool,
        has_object_id: bool,
        is_payload: bool,
    ) -> anyhow::Result<u64> {
        if !self.end_of_group && !has_extension_headers && has_object_id && is_payload {
            Ok(0x00)
        } else if !self.end_of_group && has_extension_headers && has_object_id && is_payload {
            Ok(0x01)
        } else if self.end_of_group && !has_extension_headers && has_object_id && is_payload {
            Ok(0x02)
        } else if self.end_of_group && has_extension_headers && has_object_id && is_payload {
            Ok(0x03)
        } else if !self.end_of_group && !has_extension_headers && !has_object_id && is_payload {
            Ok(0x04)
        } else if !self.end_of_group && has_extension_headers && !has_object_id && is_payload {
            Ok(0x05)
        } else if self.end_of_group && !has_extension_headers && !has_object_id && is_payload {
            Ok(0x06)
        } else if self.end_of_group && has_extension_headers && !has_object_id && is_payload {
            Ok(0x07)
        } else if !self.end_of_group && !has_extension_headers && has_object_id && !is_payload {
            Ok(0x20)
        } else if !self.end_of_group && has_extension_headers && has_object_id && !is_payload {
            Ok(0x21)
        } else {
            tracing::error!("Invalid message type");
            bail!("Invalid message type")
        }
    }

    fn create_extension_headers(
        prior_group_id_gap: Option<u64>,
        prior_object_id_gap: Option<u64>,
        immutable_extensions: Vec<u8>,
    ) -> Vec<KeyValuePair> {
        let mut extension_headers = Vec::new();
        if let Some(prior_object_id_gap) = prior_object_id_gap {
            let extension_header = KeyValuePair {
                key: ExtensionHeaderType::PriorObjectIdGap as u64,
                value: VariantType::Even(prior_object_id_gap),
            };
            extension_headers.push(extension_header);
        }
        if let Some(prior_group_id_gap) = prior_group_id_gap {
            let extension_header = KeyValuePair {
                key: ExtensionHeaderType::PriorGroupIdGap as u64,
                value: VariantType::Even(prior_group_id_gap),
            };
            extension_headers.push(extension_header);
        }
        if !immutable_extensions.is_empty() {
            let extension_header = KeyValuePair {
                key: ExtensionHeaderType::ImmutableExtensions as u64,
                value: VariantType::Odd(immutable_extensions),
            };
            extension_headers.push(extension_header);
        }
        extension_headers
    }

    pub fn send(&self, object: DatagramObject) {
        let bytes = object.packetize();
        self.session_context
            .transport_connection
            .send_datagram(bytes)
            .unwrap();
    }
}

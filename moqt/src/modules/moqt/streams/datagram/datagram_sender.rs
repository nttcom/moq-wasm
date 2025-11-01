use std::sync::Arc;

use anyhow::bail;
use bytes::{Bytes, BytesMut};

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

pub struct DatagramHeader {
    pub group_id: u64,
    pub object_id: Option<u64>,
    pub publisher_priority: u8,
    pub prior_object_id_gap: Option<u64>,
    pub prior_group_id_gap: Option<u64>,
    pub immutable_extensions: Vec<u8>,
}

pub struct DatagramSender<T: TransportProtocol> {
    pub track_alias: u64,
    pub end_of_group: bool,
    session_context: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> DatagramSender<T> {
    pub(crate) fn new(track_alias: u64, session_context: Arc<SessionContext<T>>) -> Self {
        Self {
            track_alias,
            end_of_group: false,
            session_context,
        }
    }

    pub fn create_object_datagram(
        &self,
        datagram_header: DatagramHeader,
        bytes: &[u8],
    ) -> anyhow::Result<DatagramObject> {
        let (object_id, has_object_id) = if let Some(object_id) = datagram_header.object_id {
            (object_id, true)
        } else {
            (0, false)
        };
        let has_extension_headers = datagram_header.prior_group_id_gap.is_some()
            || datagram_header.prior_object_id_gap.is_some()
            || !datagram_header.immutable_extensions.is_empty();
        let extension_headers = if has_extension_headers {
            Self::create_extension_headers(
                datagram_header.prior_group_id_gap,
                datagram_header.prior_object_id_gap,
                datagram_header.immutable_extensions,
            )
        } else {
            vec![]
        };

        let message_type = self.fix_message_type(has_extension_headers, has_object_id, true)?;
        Ok(DatagramObject {
            message_type,
            track_alias: self.track_alias,
            group_id: datagram_header.group_id,
            object_id,
            publisher_priority: datagram_header.publisher_priority,
            extension_headers,
            object_status: None,
            object_payload: Bytes::copy_from_slice(bytes),
        })
    }

    pub fn create_object_datagram_from_binary(
        &self,
        mut binary_object: BytesMut,
    ) -> anyhow::Result<DatagramObject> {
        let mut object = DatagramObject::depacketize(&mut binary_object)?;
        object.track_alias = self.track_alias;
        Ok(object)
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

    pub async fn send(&self, object: DatagramObject) -> anyhow::Result<()> {
        let bytes = object.packetize();
        let result = self
            .session_context
            .transport_connection
            .send_datagram(bytes);
        tokio::task::yield_now().await;
        result
    }

    pub fn overwrite_track_alias_then_send(
        &self,
        mut object: DatagramObject,
    ) -> anyhow::Result<()> {
        object.track_alias = self.track_alias;
        let bytes = object.packetize();
        self.session_context
            .transport_connection
            .send_datagram(bytes)
    }
}

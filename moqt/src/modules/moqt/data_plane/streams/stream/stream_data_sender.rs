use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            data_plane::{
                object::{
                    extension_headers::ExtensionHeaders,
                    subgroup::{SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField},
                },
                streams::stream::stream_sender::StreamSender,
            },
            domains::session_context::SessionContext,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct StreamDataSender<T: TransportProtocol> {
    stream_sender: StreamSender<T>,
    track_alias: u64,
    subgroup_header: Option<SubgroupHeader>,
}

impl<T: TransportProtocol> StreamDataSender<T> {
    pub(crate) async fn new(
        track_alias: u64,
        session_context: Arc<SessionContext<T>>,
    ) -> anyhow::Result<Self> {
        let stream = session_context.transport_connection.open_uni().await?;
        let stream_sender = StreamSender::new(stream);
        Ok(Self {
            stream_sender,
            track_alias,
            subgroup_header: None,
        })
    }

    pub fn create_header(
        &self,
        group_id: u64,
        subgroup_id: SubgroupId,
        publisher_priority: u8,
        end_of_group: bool,
        has_extensions: bool,
    ) -> SubgroupHeader {
        SubgroupHeader::new(
            self.track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
            end_of_group,
            has_extensions,
        )
    }

    pub fn create_object_field(
        &self,
        header: &SubgroupHeader,
        object_id_delta: u64,
        extension_headers: ExtensionHeaders,
        subgroup_object: SubgroupObject,
    ) -> SubgroupObjectField {
        SubgroupObjectField {
            message_type: header.message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        }
    }

    pub async fn send(
        &mut self,
        header: &SubgroupHeader,
        data: SubgroupObjectField,
    ) -> anyhow::Result<()> {
        if self.subgroup_header.is_none() {
            tracing::debug!("Sending new subgroup header: {:?}", header);
            let encoded_header = header.encode();
            self.stream_sender.send(&encoded_header).await?;
            self.subgroup_header = Some(header.clone());
        }
        tracing::debug!("Sending subgroup object");
        let bytes = data.encode();
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }
}

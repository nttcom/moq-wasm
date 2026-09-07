use anyhow::Context;
use bytes::Bytes;

use crate::{
    ExtensionHeaders, ObjectStatus, StreamDataSenderFactory, SubgroupId, SubgroupObject,
    SubgroupObjectSender, TransportProtocol,
};

const PUBLISHER_PRIORITY: u8 = 128;

pub struct TrackWriter<T: TransportProtocol> {
    factory: StreamDataSenderFactory<T>,
    first_group_id: u64,
    next_group_id: u64,
    group: Option<GroupSender<T>>,
}

impl<T: TransportProtocol> TrackWriter<T> {
    pub fn new(factory: StreamDataSenderFactory<T>, first_group_id: u64) -> Self {
        Self {
            factory,
            first_group_id,
            next_group_id: first_group_id,
            group: None,
        }
    }

    pub async fn start_group(&mut self) -> anyhow::Result<()> {
        self.finish_current_group().await?;
        self.group = Some(self.open_group().await?);
        Ok(())
    }

    pub async fn write(
        &mut self,
        payload: Bytes,
        immutable_extensions: Vec<Bytes>,
    ) -> anyhow::Result<()> {
        self.group
            .as_mut()
            .context("write before start_group")?
            .write_object(payload, immutable_extensions)
            .await
    }

    pub async fn write_group(&mut self, payload: Bytes) -> anyhow::Result<()> {
        self.finish_current_group().await?;
        let mut group = self.open_group().await?;
        group.write_object(payload, vec![]).await?;
        group.finish().await
    }

    pub async fn finish(mut self) -> anyhow::Result<()> {
        self.finish_current_group().await
    }

    pub fn groups(&self) -> u64 {
        self.next_group_id - self.first_group_id
    }

    async fn finish_current_group(&mut self) -> anyhow::Result<()> {
        match self.group.take() {
            Some(group) => group.finish().await,
            None => Ok(()),
        }
    }

    async fn open_group(&mut self) -> anyhow::Result<GroupSender<T>> {
        let uninitialized = self.factory.next().await?;
        let header = uninitialized.create_header(
            self.next_group_id,
            SubgroupId::None,
            PUBLISHER_PRIORITY,
            false,
            false,
        );
        let sender = uninitialized.send_header(header).await?;
        self.next_group_id += 1;
        Ok(GroupSender { sender })
    }
}

struct GroupSender<T: TransportProtocol> {
    sender: SubgroupObjectSender<T>,
}

impl<T: TransportProtocol> GroupSender<T> {
    async fn write_object(
        &mut self,
        payload: Bytes,
        immutable_extensions: Vec<Bytes>,
    ) -> anyhow::Result<()> {
        let extension_headers = ExtensionHeaders::from_immutable_extensions(immutable_extensions);
        let field = self.sender.create_object_field(
            0,
            extension_headers,
            SubgroupObject::new_payload(payload),
        );
        self.sender.send(field).await
    }

    async fn finish(mut self) -> anyhow::Result<()> {
        let end_of_group = self.sender.create_object_field(
            0,
            ExtensionHeaders::default(),
            SubgroupObject::new_status(ObjectStatus::EndOfGroup as u64),
        );
        self.sender.send(end_of_group).await?;
        self.sender.close().await
    }
}

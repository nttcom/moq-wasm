use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    ExtensionHeaders, ObjectStatus, QUIC, StreamDataSenderFactory, SubgroupId, SubgroupObject,
    SubgroupObjectSender,
};

const PUBLISHER_PRIORITY: u8 = 128;

/// Writes objects to one published track, one group (GOP) at a time.
/// The only place that touches moqt's object-writing API on the publish side.
pub struct TrackWriter {
    factory: StreamDataSenderFactory<QUIC>,
    next_group_id: u64,
    group: Option<GroupSender>,
}

impl TrackWriter {
    pub fn new(factory: StreamDataSenderFactory<QUIC>) -> Self {
        Self {
            factory,
            next_group_id: 0,
            group: None,
        }
    }

    /// Close the current group (EndOfGroup) and open a new one.
    pub async fn start_group(&mut self) -> Result<()> {
        if let Some(group) = self.group.take() {
            group.finish().await?;
        }
        self.group = Some(self.open_group().await?);
        Ok(())
    }

    /// Write one object into the current group, with its LOC header extensions.
    pub async fn write(&mut self, object: Bytes, immutable_extensions: Vec<Bytes>) -> Result<()> {
        self.group
            .as_mut()
            .context("write before start_group")?
            .write_object(object, immutable_extensions)
            .await
    }

    /// Write a single object as its own, immediately-closed group (catalog snapshots).
    pub async fn write_group(&mut self, object: Bytes) -> Result<()> {
        if let Some(group) = self.group.take() {
            group.finish().await?;
        }
        let mut group = self.open_group().await?;
        group.write_object(object, vec![]).await?;
        group.finish().await
    }

    /// Close the last open group.
    pub async fn finish(mut self) -> Result<()> {
        if let Some(group) = self.group.take() {
            group.finish().await?;
        }
        Ok(())
    }

    pub fn groups(&self) -> u64 {
        self.next_group_id
    }

    async fn open_group(&mut self) -> Result<GroupSender> {
        let uninit = self.factory.next().await?;
        let header = uninit.create_header(
            self.next_group_id,
            SubgroupId::None,
            PUBLISHER_PRIORITY,
            false,
            false,
        );
        let sender = uninit.send_header(header).await?;
        self.next_group_id += 1;
        Ok(GroupSender { sender })
    }
}

/// A single open group's object stream.
struct GroupSender {
    sender: SubgroupObjectSender<QUIC>,
}

impl GroupSender {
    async fn write_object(
        &mut self,
        object: Bytes,
        immutable_extensions: Vec<Bytes>,
    ) -> Result<()> {
        let ext = ExtensionHeaders::from_immutable_extensions(immutable_extensions);
        let field = self
            .sender
            .create_object_field(0, ext, SubgroupObject::new_payload(object));
        self.sender.send(field).await
    }

    async fn finish(mut self) -> Result<()> {
        let end_of_group = self.sender.create_object_field(
            0,
            empty_ext(),
            SubgroupObject::new_status(ObjectStatus::EndOfGroup as u64),
        );
        self.sender.send(end_of_group).await?;
        self.sender.close().await
    }
}

fn empty_ext() -> ExtensionHeaders {
    ExtensionHeaders::default()
}

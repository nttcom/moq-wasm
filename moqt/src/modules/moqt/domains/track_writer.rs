use anyhow::{Context, ensure};
use bytes::Bytes;

use crate::{
    ExtensionHeaders, ObjectStatus, StreamDataSenderFactory, SubgroupId, SubgroupObject,
    SubgroupObjectSender, TransportProtocol,
};

const PUBLISHER_PRIORITY: u8 = 128;
/// draft-14 §10.4.2: extension headers are only encoded when the subgroup
/// header type says they are present, and `write()` accepts extensions per
/// object, so every subgroup declares them (objects without any carry a
/// zero-length extension block).
const EXTENSIONS_PRESENT: bool = true;

pub struct TrackWriter<T: TransportProtocol> {
    factory: StreamDataSenderFactory<T>,
    first_group_id: u64,
    next_group_id: u64,
    group: Option<GroupSender<T>>,
    pending_group_gap: Option<u64>,
}

impl<T: TransportProtocol> TrackWriter<T> {
    pub fn new(factory: StreamDataSenderFactory<T>, first_group_id: u64) -> Self {
        Self {
            factory,
            first_group_id,
            next_group_id: first_group_id,
            group: None,
            pending_group_gap: None,
        }
    }

    pub async fn start_group(&mut self) -> anyhow::Result<()> {
        self.finish_current_group().await?;
        self.group = Some(self.open_group().await?);
        Ok(())
    }

    /// Opens a group with a chosen id so tracks of a switching set can start
    /// their groups at the same ids. Ids skipped over are announced on the new
    /// group's first object with the Prior Group ID Gap extension header of
    /// draft-ietf-moq-transport-14; a writer that has not opened a group yet
    /// may start anywhere.
    pub async fn start_group_at(&mut self, group_id: u64) -> anyhow::Result<()> {
        if self.groups() == 0 {
            self.first_group_id = group_id;
        } else {
            ensure!(
                group_id >= self.next_group_id,
                "group {group_id} precedes the next group {}",
                self.next_group_id
            );
            if group_id > self.next_group_id {
                self.pending_group_gap = Some(group_id - self.next_group_id);
            }
        }
        self.next_group_id = group_id;
        self.start_group().await
    }

    pub async fn write(
        &mut self,
        payload: Bytes,
        immutable_extensions: Vec<Bytes>,
    ) -> anyhow::Result<()> {
        self.write_with_extension_headers(
            payload,
            ExtensionHeaders::from_immutable_extensions(immutable_extensions),
        )
        .await
    }

    pub async fn write_with_extension_headers(
        &mut self,
        payload: Bytes,
        mut extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<()> {
        if let Some(gap) = self.pending_group_gap.take() {
            extension_headers.push_prior_group_id_gap(gap);
        }
        self.group
            .as_mut()
            .context("write before start_group")?
            .write_object(payload, extension_headers)
            .await
    }

    pub async fn write_group(&mut self, payload: Bytes) -> anyhow::Result<()> {
        self.finish_current_group().await?;
        let mut group = self.open_group().await?;
        group
            .write_object(payload, ExtensionHeaders::default())
            .await?;
        group.finish().await
    }

    pub async fn finish(mut self) -> anyhow::Result<()> {
        self.finish_current_group().await
    }

    pub fn groups(&self) -> u64 {
        self.next_group_id - self.first_group_id
    }

    pub fn next_group_id(&self) -> u64 {
        self.next_group_id
    }

    pub fn current_group_id(&self) -> Option<u64> {
        self.group.as_ref().map(|_| self.next_group_id - 1)
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
            EXTENSIONS_PRESENT,
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
        extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        ExtensionHeaders, KeyValuePair, PublishOption, TrackWriter, VariantType,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, accept_publish, connect_sessions, spawn_dual_server,
            subscribed_track_reader,
        },
    };

    #[tokio::test]
    async fn objects_arrive_with_their_immutable_extensions_and_ids() {
        // Arrange
        let (port, accept) = spawn_dual_server("track-writer");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let publisher = client.publisher();
        let (published, accepted) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            tokio::join!(
                publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                accept_publish(&server)
            )
        })
        .await
        .unwrap();
        let mut writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), 7);

        // Act
        writer.start_group().await.unwrap();
        writer
            .write(
                Bytes::from_static(b"with"),
                vec![Bytes::from_static(b"ext")],
            )
            .await
            .unwrap();
        writer
            .write(Bytes::from_static(b"without"), vec![])
            .await
            .unwrap();
        writer
            .write_with_extension_headers(
                Bytes::from_static(b"keyed"),
                ExtensionHeaders::new(vec![KeyValuePair {
                    key: 2,
                    value: VariantType::Even(42),
                }]),
            )
            .await
            .unwrap();
        let current_group_id = writer.current_group_id();
        writer.finish().await.unwrap();
        let mut reader = subscribed_track_reader(&server, &accepted).await;
        let first = reader.next_object().await.unwrap().unwrap();
        let second = reader.next_object().await.unwrap().unwrap();
        let third = reader.next_object().await.unwrap().unwrap();

        // Assert
        assert_eq!(current_group_id, Some(7));
        assert_eq!((first.group_id, first.object_id), (7, 0));
        assert_eq!(first.payload, Bytes::from_static(b"with"));
        assert_eq!(
            first.extension_headers.immutable_extensions(),
            vec![Bytes::from_static(b"ext")]
        );
        assert_eq!((second.group_id, second.object_id), (7, 1));
        assert_eq!(second.payload, Bytes::from_static(b"without"));
        assert!(second.extension_headers.immutable_extensions().is_empty());
        assert_eq!((third.group_id, third.object_id), (7, 2));
        assert_eq!(
            third.extension_headers.key_value_pairs,
            vec![KeyValuePair {
                key: 2,
                value: VariantType::Even(42),
            }]
        );
    }

    #[tokio::test]
    async fn chosen_group_ids_announce_the_ids_they_skip() {
        // Arrange
        let (port, accept) = spawn_dual_server("track-writer-groups");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let publisher = client.publisher();
        let (published, accepted) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            tokio::join!(
                publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                accept_publish(&server)
            )
        })
        .await
        .unwrap();
        let mut writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), 7);

        // Act: the first group may start anywhere, the next one skips 4 and 5
        writer.start_group_at(3).await.unwrap();
        writer
            .write(Bytes::from_static(b"a"), vec![])
            .await
            .unwrap();
        writer.start_group_at(6).await.unwrap();
        writer
            .write(Bytes::from_static(b"b"), vec![])
            .await
            .unwrap();
        writer
            .write(Bytes::from_static(b"c"), vec![])
            .await
            .unwrap();
        let next_group_id = writer.next_group_id();
        writer.finish().await.unwrap();
        let mut reader = subscribed_track_reader(&server, &accepted).await;
        let first = reader.next_object().await.unwrap().unwrap();
        let second = reader.next_object().await.unwrap().unwrap();
        let third = reader.next_object().await.unwrap().unwrap();

        // Assert
        assert_eq!(next_group_id, 7);
        assert_eq!((first.group_id, first.object_id), (3, 0));
        assert!(first.extension_headers.prior_group_id_gap().is_empty());
        assert_eq!((second.group_id, second.object_id), (6, 0));
        assert_eq!(second.extension_headers.prior_group_id_gap(), vec![2]);
        assert_eq!((third.group_id, third.object_id), (6, 1));
        assert!(third.extension_headers.prior_group_id_gap().is_empty());
    }
}

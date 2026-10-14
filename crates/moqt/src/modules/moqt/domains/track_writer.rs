use anyhow::{Context, ensure};
use bytes::Bytes;

use crate::{
    object::{
        ExtensionHeaders, ForwardingPreference, Object, ObjectStatus, StreamDataSenderFactory,
        SubgroupObjectSender, SubgroupStart,
    },
    wire::Location,
};

const PUBLISHER_PRIORITY: u8 = 128;

pub struct TrackWriter {
    factory: StreamDataSenderFactory,
    first_group_id: u64,
    next_group_id: u64,
    group: Option<GroupSender>,
    pending_group_gap: Option<u64>,
}

impl TrackWriter {
    pub fn new(factory: StreamDataSenderFactory, first_group_id: u64) -> Self {
        Self {
            factory,
            first_group_id,
            next_group_id: first_group_id,
            group: None,
            pending_group_gap: None,
        }
    }

    pub async fn start_group(&mut self) -> anyhow::Result<()> {
        self.finish_current_group().await;
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
        let result = self
            .group
            .as_mut()
            .context("write before start_group")?
            .write_object(payload, extension_headers)
            .await;
        if result.is_err() {
            self.group = None;
        }
        result
    }

    pub async fn write_group(&mut self, payload: Bytes) -> anyhow::Result<()> {
        self.finish_current_group().await;
        let mut group = self.open_group().await?;
        group
            .write_object(payload, ExtensionHeaders::default())
            .await?;
        group.finish().await
    }

    pub async fn finish(mut self) -> anyhow::Result<()> {
        match self.group.take() {
            Some(group) => group.finish().await,
            None => Ok(()),
        }
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

    /// A group the subscriber stopped or the transport reset cannot be
    /// finished, and its stream is gone either way, so the next group opens
    /// on a new stream regardless.
    async fn finish_current_group(&mut self) {
        if let Some(group) = self.group.take()
            && let Err(error) = group.finish().await
        {
            tracing::debug!(?error, "previous group could not be finished");
        }
    }

    async fn open_group(&mut self) -> anyhow::Result<GroupSender> {
        let group_id = self.next_group_id;
        let sender = self
            .factory
            .next()
            .await?
            .start(SubgroupStart {
                group_id,
                subgroup_id: 0,
                publisher_priority: PUBLISHER_PRIORITY,
                ends_group: false,
            })
            .await?;
        self.next_group_id += 1;
        Ok(GroupSender {
            sender,
            group_id,
            next_object_id: 0,
        })
    }
}

struct GroupSender {
    sender: SubgroupObjectSender,
    group_id: u64,
    next_object_id: u64,
}

impl GroupSender {
    async fn write_object(
        &mut self,
        payload: Bytes,
        extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<()> {
        self.send(ObjectStatus::Normal, extension_headers, payload)
            .await
    }

    async fn finish(mut self) -> anyhow::Result<()> {
        self.send(
            ObjectStatus::EndOfGroup,
            ExtensionHeaders::default(),
            Bytes::new(),
        )
        .await?;
        self.sender.close().await
    }

    async fn send(
        &mut self,
        status: ObjectStatus,
        extension_headers: ExtensionHeaders,
        payload: Bytes,
    ) -> anyhow::Result<()> {
        let object = Object {
            location: Location {
                group_id: self.group_id,
                object_id: self.next_object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: PUBLISHER_PRIORITY,
            status,
            extension_headers,
            payload,
            ends_group: false,
        };
        self.sender.send_object(&object).await?;
        self.next_object_id += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        TrackWriter,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, accept_publish, connect_sessions, spawn_dual_server,
            subscribed_track_reader,
        },
        object::{DataReceiver, ExtensionHeaders, KeyValuePair, Subgroup, VariantType},
        publish::PublishOption,
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

    #[tokio::test]
    async fn a_group_the_subscriber_stopped_does_not_keep_the_next_group_from_opening() {
        // Arrange
        let (port, accept) = spawn_dual_server("track-writer-stopped-group");
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
        writer.start_group().await.unwrap();
        writer
            .write(Bytes::from_static(b"a"), vec![])
            .await
            .unwrap();
        let DataReceiver::Stream(mut streams) = server
            .subscriber()
            .accept_data_receiver(&accepted)
            .await
            .unwrap()
        else {
            panic!("expected a subgroup stream receiver");
        };
        drop(streams.next().await.unwrap());
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            while writer
                .write(Bytes::from_static(b"after stop"), vec![])
                .await
                .is_ok()
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        // Act
        writer.start_group().await.unwrap();
        writer
            .write(Bytes::from_static(b"b"), vec![])
            .await
            .unwrap();
        let mut next_stream = streams.next().await.unwrap();
        let header = next_stream.receive().await.unwrap();
        let object = next_stream.receive().await.unwrap();

        // Assert
        let Some(Subgroup::Header(header)) = header else {
            panic!("expected the next group's subgroup header");
        };
        assert_eq!(header.group_id, 8);
        let Some(Subgroup::Object(object)) = object else {
            panic!("expected the next group's first object");
        };
        assert_eq!(object.payload, Bytes::from_static(b"b"));
    }
}

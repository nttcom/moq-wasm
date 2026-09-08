use anyhow::Context;
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        DUAL, DataReceiver, FilterType, PublishOption, Session, SessionEvent, Subscription,
        TrackReader, TrackWriter,
        modules::test_support::{HANDSHAKE_TIMEOUT, connect_sessions, spawn_dual_server},
    };

    async fn accept_publish(server: &Session<DUAL>) -> Subscription {
        let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };
        let subscription = handler.ok(128, FilterType::LargestObject, 0).await.unwrap();
        handler.accept_data_receiver().await;
        subscription
    }

    /// The data receiver resolves only once the first object has arrived, so
    /// this must run after the writer has sent something.
    async fn track_reader(
        server: &Session<DUAL>,
        subscription: &Subscription,
    ) -> TrackReader<DUAL> {
        let DataReceiver::Stream(factory) = server
            .subscriber()
            .accept_data_receiver(subscription)
            .await
            .unwrap()
        else {
            panic!("expected a subgroup stream receiver");
        };
        TrackReader::new(factory)
    }

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
        writer.finish().await.unwrap();
        let mut reader = track_reader(&server, &accepted).await;
        let first = reader.next_object().await.unwrap().unwrap();
        let second = reader.next_object().await.unwrap().unwrap();

        // Assert
        assert_eq!((first.group_id, first.object_id), (7, 0));
        assert_eq!(first.payload, Bytes::from_static(b"with"));
        assert_eq!(
            first.extension_headers.immutable_extensions(),
            vec![Bytes::from_static(b"ext")]
        );
        assert_eq!((second.group_id, second.object_id), (7, 1));
        assert_eq!(second.payload, Bytes::from_static(b"without"));
        assert!(second.extension_headers.immutable_extensions().is_empty());
    }
}

use std::marker::PhantomData;

use bytes::Bytes;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

use crate::{
    ExtensionHeaders, StreamDataReceiver, StreamDataReceiverFactory, Subgroup, SubgroupHeader,
    SubgroupId, SubgroupObject, TransportProtocol,
};

/// Bounds how far a reader task runs ahead of `next_object()`; a full channel
/// stops reading that QUIC stream so flow control reaches the publisher.
const OBJECT_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct TrackObject {
    pub group_id: u64,
    pub subgroup_id: u64,
    pub object_id: u64,
    pub extension_headers: ExtensionHeaders,
    pub payload: Bytes,
}

/// Reads every subgroup stream of one track concurrently and yields objects in
/// arrival order. Ordering is only guaranteed within a subgroup (draft-14
/// §10.4); consumers that need cross-subgroup order use `group_id`,
/// `subgroup_id` and `object_id`.
pub struct TrackReader<T: TransportProtocol> {
    object_receiver: mpsc::Receiver<anyhow::Result<TrackObject>>,
    _stream_accept_task: SubgroupStreamAcceptTask,
    _protocol: PhantomData<T>,
}

impl<T: TransportProtocol> TrackReader<T> {
    pub fn new(factory: StreamDataReceiverFactory<T>) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        Self {
            object_receiver,
            _stream_accept_task: SubgroupStreamAcceptTask::run(factory, object_sender),
            _protocol: PhantomData,
        }
    }

    /// Returns `Ok(None)` once the track has no more subgroup streams. A
    /// failure inside one subgroup stream is returned as `Err`; the other
    /// streams keep being read.
    pub async fn next_object(&mut self) -> anyhow::Result<Option<TrackObject>> {
        self.object_receiver.recv().await.transpose()
    }
}

struct SubgroupStreamAcceptTask {
    join_handle: JoinHandle<()>,
}

impl SubgroupStreamAcceptTask {
    fn run<T: TransportProtocol>(
        mut factory: StreamDataReceiverFactory<T>,
        object_sender: mpsc::Sender<anyhow::Result<TrackObject>>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut subgroup_readers = JoinSet::new();
            loop {
                match factory.next().await {
                    Ok(receiver) => {
                        subgroup_readers.spawn(read_subgroup(receiver, object_sender.clone()));
                    }
                    Err(error) => {
                        tracing::debug!(%error, "track ended");
                        break;
                    }
                }
            }
            drop(object_sender);
            while subgroup_readers.join_next().await.is_some() {}
        });
        Self { join_handle }
    }
}

impl Drop for SubgroupStreamAcceptTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn read_subgroup<T: TransportProtocol>(
    mut receiver: StreamDataReceiver<T>,
    object_sender: mpsc::Sender<anyhow::Result<TrackObject>>,
) {
    if let Err(error) = read_subgroup_objects(&mut receiver, &object_sender).await {
        let _ = object_sender.send(Err(error)).await;
    }
}

async fn read_subgroup_objects<T: TransportProtocol>(
    receiver: &mut StreamDataReceiver<T>,
    object_sender: &mpsc::Sender<anyhow::Result<TrackObject>>,
) -> anyhow::Result<()> {
    let header = match receiver.receive().await? {
        Some(Subgroup::Header(header)) => header,
        other => anyhow::bail!("subgroup stream did not start with a header: {other:?}"),
    };
    let mut prev_object_id = None;
    loop {
        match receiver.receive().await? {
            Some(Subgroup::Object(field)) => {
                let object_id = field.resolve_object_id(prev_object_id);
                let first_object_id = prev_object_id.unwrap_or(object_id);
                prev_object_id = Some(object_id);
                let SubgroupObject::Payload { data, .. } = field.subgroup_object else {
                    continue;
                };
                let object = TrackObject {
                    group_id: header.group_id,
                    subgroup_id: resolve_subgroup_id(&header, first_object_id),
                    object_id,
                    extension_headers: field.extension_headers,
                    payload: data,
                };
                if object_sender.send(Ok(object)).await.is_err() {
                    return Ok(());
                }
            }
            Some(Subgroup::Header(_)) => continue,
            None => return Ok(()),
        }
    }
}

/// draft-14 §10.4.1: header types without a Subgroup ID field mean subgroup
/// 0, and the "first object id" types take the id of the first object sent.
fn resolve_subgroup_id(header: &SubgroupHeader, first_object_id: u64) -> u64 {
    match header.subgroup_id {
        SubgroupId::None => 0,
        SubgroupId::Value(subgroup_id) => subgroup_id,
        SubgroupId::FirstObjectIdDelta => first_object_id,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, time::Duration};

    use bytes::Bytes;

    use crate::{
        DUAL, ExtensionHeaders, PublishOption, StreamDataSenderFactory, SubgroupId, SubgroupObject,
        SubgroupObjectSender, TrackObject, TrackReader,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, accept_publish, connect_sessions, spawn_dual_server,
            subscribed_track_reader,
        },
    };

    const PUBLISHER_PRIORITY: u8 = 128;
    const NO_OBJECT_TIMEOUT: Duration = Duration::from_millis(500);

    async fn open_subgroup(
        factory: &StreamDataSenderFactory<DUAL>,
        group_id: u64,
        subgroup_id: SubgroupId,
    ) -> SubgroupObjectSender<DUAL> {
        let uninitialized = factory.next().await.unwrap();
        let header =
            uninitialized.create_header(group_id, subgroup_id, PUBLISHER_PRIORITY, false, false);
        uninitialized.send_header(header).await.unwrap()
    }

    async fn send_payload(sender: &mut SubgroupObjectSender<DUAL>, payload: &'static [u8]) {
        let field = sender.create_object_field(
            0,
            ExtensionHeaders::default(),
            SubgroupObject::new_payload(Bytes::from_static(payload)),
        );
        sender.send(field).await.unwrap();
    }

    async fn collect_objects(reader: &mut TrackReader<DUAL>, count: usize) -> Vec<TrackObject> {
        let mut objects = Vec::new();
        while objects.len() < count {
            let object = tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.next_object())
                .await
                .expect("object within timeout")
                .unwrap()
                .expect("track still open");
            objects.push(object);
        }
        objects
    }

    fn locations(objects: &[TrackObject]) -> BTreeSet<(u64, u64, u64, Bytes)> {
        objects
            .iter()
            .map(|object| {
                (
                    object.group_id,
                    object.subgroup_id,
                    object.object_id,
                    object.payload.clone(),
                )
            })
            .collect()
    }

    async fn published_track(
        name: &str,
    ) -> (
        crate::Session<DUAL>,
        crate::Session<DUAL>,
        StreamDataSenderFactory<DUAL>,
        crate::Subscription,
    ) {
        let (port, accept) = spawn_dual_server(name);
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
        let factory = publisher.create_stream(&published.unwrap());
        (client, server, factory, accepted)
    }

    #[tokio::test]
    async fn next_group_is_read_while_the_previous_stream_is_still_open() {
        // Arrange
        let (_client, server, factory, subscription) = published_track("reader-unclosed").await;
        let mut group_0 = open_subgroup(&factory, 0, SubgroupId::None).await;
        send_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&server, &subscription).await;

        // Act
        let mut group_1 = open_subgroup(&factory, 1, SubgroupId::None).await;
        send_payload(&mut group_1, b"g1-o0").await;
        send_payload(&mut group_0, b"g0-o1").await;
        let objects = collect_objects(&mut reader, 3).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (0, 0, 0, Bytes::from_static(b"g0-o0")),
                (0, 0, 1, Bytes::from_static(b"g0-o1")),
                (1, 0, 0, Bytes::from_static(b"g1-o0")),
            ])
        );
    }

    #[tokio::test]
    async fn subgroups_of_one_group_are_read_concurrently() {
        // Arrange
        let (_client, server, factory, subscription) = published_track("reader-subgroups").await;
        let mut subgroup_0 = open_subgroup(&factory, 5, SubgroupId::Value(0)).await;
        send_payload(&mut subgroup_0, b"s0-o0").await;
        let mut reader = subscribed_track_reader(&server, &subscription).await;

        // Act
        let mut subgroup_1 = open_subgroup(&factory, 5, SubgroupId::Value(1)).await;
        send_payload(&mut subgroup_1, b"s1-o0").await;
        send_payload(&mut subgroup_0, b"s0-o1").await;
        let objects = collect_objects(&mut reader, 3).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (5, 0, 0, Bytes::from_static(b"s0-o0")),
                (5, 0, 1, Bytes::from_static(b"s0-o1")),
                (5, 1, 0, Bytes::from_static(b"s1-o0")),
            ])
        );
    }

    #[tokio::test]
    async fn first_object_id_subgroup_type_takes_the_first_object_id() {
        // Arrange
        let (_client, server, factory, subscription) = published_track("reader-first-id").await;
        let mut subgroup = open_subgroup(&factory, 2, SubgroupId::FirstObjectIdDelta).await;
        let field = subgroup.create_object_field(
            7,
            ExtensionHeaders::default(),
            SubgroupObject::new_payload(Bytes::from_static(b"o7")),
        );
        subgroup.send(field).await.unwrap();
        let mut reader = subscribed_track_reader(&server, &subscription).await;

        // Act
        send_payload(&mut subgroup, b"o8").await;
        let objects = collect_objects(&mut reader, 2).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (2, 7, 7, Bytes::from_static(b"o7")),
                (2, 7, 8, Bytes::from_static(b"o8")),
            ])
        );
    }

    #[tokio::test]
    async fn status_objects_are_skipped_and_closing_a_stream_keeps_the_track_open() {
        // Arrange
        let (_client, server, factory, subscription) = published_track("reader-status").await;
        let mut group_0 = open_subgroup(&factory, 0, SubgroupId::None).await;
        send_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&server, &subscription).await;

        // Act
        let end_of_group = group_0.create_object_field(
            0,
            ExtensionHeaders::default(),
            SubgroupObject::new_status(crate::ObjectStatus::EndOfGroup as u64),
        );
        group_0.send(end_of_group).await.unwrap();
        group_0.close().await.unwrap();
        let objects = collect_objects(&mut reader, 1).await;
        let nothing_more = tokio::time::timeout(NO_OBJECT_TIMEOUT, reader.next_object()).await;

        // Assert
        assert_eq!(objects[0].payload, Bytes::from_static(b"g0-o0"));
        assert!(nothing_more.is_err(), "reader must wait for further groups");
    }
}

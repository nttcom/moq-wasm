use std::time::Duration;

use bytes::Bytes;
use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;

use crate::{
    RequestFailure, Subscriber, Subscription,
    fetch::{Fetch, FetchHandle, FetchOption},
    modules::{
        executor::{self, AbortOnDrop, MaybeSend},
        moqt::domains::subscriber::{SentFetch, joining_remainder},
    },
    object::{
        ContentExists, DataReceiver, DataStreamResetCode, ExtensionHeaders, ForwardingPreference,
        Location, MalformedTrackError, Object, ObjectStatus, ReadError, StreamDataReceiver,
        StreamReceiveError, Subgroup, TrackData,
    },
};

/// Bounds how far a reader task runs ahead of `next_object()`; a full channel
/// stops reading that QUIC stream so flow control reaches the publisher.
const OBJECT_CHANNEL_CAPACITY: usize = 64;

/// How long a reader that asked for a new group waits for it before it
/// fetches the current group instead: a publisher may hold the request back
/// until its group is long enough, or have no frame to encode.
const NEW_GROUP_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct TrackObject {
    pub group_id: u64,
    /// The subgroup the object arrived in, or `Datagram`.
    pub forwarding: ForwardingPreference,
    pub object_id: u64,
    pub extension_headers: ExtensionHeaders,
    pub payload: Bytes,
}

impl TrackObject {
    /// `None` for an object without media: a status or an empty payload.
    pub fn from_object(object: Object) -> Option<Self> {
        if object.status != ObjectStatus::Normal || object.payload.is_empty() {
            return None;
        }
        Some(Self {
            group_id: object.location.group_id,
            forwarding: object.forwarding,
            object_id: object.location.object_id,
            extension_headers: object.extension_headers,
            payload: object.payload,
        })
    }
}

#[derive(Debug, Clone)]
pub enum TrackEvent {
    Object(TrackObject),
    /// The publisher reset a subgroup stream of group `group_id` with `code`
    /// (draft-14 §13.1.8, draft-18 §15.10.4); its remaining objects are lost.
    Reset {
        group_id: u64,
        code: DataStreamResetCode,
    },
    /// The End of Group status object (draft-14 §10.2.1.1): no object of the
    /// group comes after `location`'s. `forwarding` is the subgroup it
    /// arrived in, or `Datagram`.
    EndOfGroup {
        location: Location,
        forwarding: ForwardingPreference,
    },
}

impl TrackEvent {
    /// `None` for an object that is neither media nor End of Group.
    fn from_object(object: Object) -> Option<Self> {
        if object.status == ObjectStatus::EndOfGroup && object.payload.is_empty() {
            return Some(Self::EndOfGroup {
                location: object.location,
                forwarding: object.forwarding,
            });
        }
        TrackObject::from_object(object).map(Self::Object)
    }
}

/// Reads every subgroup stream and datagram of one track concurrently,
/// inside one task, and yields objects in arrival order. Ordering is only
/// guaranteed within a subgroup (draft-14 §10.4); consumers that need
/// cross-subgroup order use `group_id`, `forwarding` and `object_id`.
pub struct TrackReader {
    object_receiver: mpsc::Receiver<anyhow::Result<TrackEvent>>,
    _track_read_task: TrackReadTask,
}

impl TrackReader {
    pub fn new(receiver: DataReceiver) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        Self {
            object_receiver,
            _track_read_task: TrackReadTask::run(async move { Ok(receiver) }, object_sender),
        }
    }

    pub(crate) fn of(subscriber: Subscriber, subscription: Subscription) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        let receiver = async move {
            let mut subscriber = subscriber;
            subscriber.accept_data_receiver(&subscription).await
        };
        Self {
            object_receiver,
            _track_read_task: TrackReadTask::run(receiver, object_sender),
        }
    }

    /// The track's data is read only after the FETCH ends, so the fetched
    /// objects come first. `joining_fetch` is the Joining FETCH written behind
    /// the SUBSCRIBE; `None` reads the live objects only.
    pub(crate) fn joining_current_group(
        subscriber: Subscriber,
        subscription: Subscription,
        joining_fetch: Option<SentFetch>,
    ) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        let fetch_sender = object_sender.clone();
        let receiver = async move {
            let mut subscriber = subscriber;
            if let Some(sent) = joining_fetch {
                let option = FetchOption::default();
                let remainder = joining_remainder(&subscription, 0, &option);
                let fetch = subscriber.fetch_answered(sent, &option, remainder).await;
                send_fetched_group(&mut subscriber, fetch, fetch_sender).await;
            }
            subscriber.accept_data_receiver(&subscription).await
        };
        Self {
            object_receiver,
            _track_read_task: TrackReadTask::run(receiver, object_sender),
        }
    }

    /// Fetches the group the subscription's Largest Object is in
    /// (`Subscriber::fetch_relative_joining`) and yields its objects before the live ones: the track's data
    /// is read only after the FETCH ends. A subscription without a Largest
    /// Object has no group to fetch, so its live objects are read at once.
    pub(crate) fn fetching_current_group(
        subscriber: Subscriber,
        subscription: Subscription,
    ) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        let fetch_sender = object_sender.clone();
        let receiver = async move {
            let mut subscriber = subscriber;
            if matches!(subscription.content_exists(), ContentExists::True { .. }) {
                let fetch = subscriber
                    .fetch_relative_joining(&subscription, 0, FetchOption::default())
                    .await;
                send_fetched_group(&mut subscriber, fetch, fetch_sender).await;
            }
            subscriber.accept_data_receiver(&subscription).await
        };
        Self {
            object_receiver,
            _track_read_task: TrackReadTask::run(receiver, object_sender),
        }
    }

    /// For a subscription that asked for a new group (draft-18 §10.2.13)
    /// while `largest_group` was the track's largest: yields the track from
    /// the first group after it. The rest of `largest_group` cannot be decoded
    /// without its start, so it is held back, and yielded behind a Joining
    /// FETCH of that group when no newer group arrives within
    /// `NEW_GROUP_WAIT`.
    pub(crate) fn awaiting_new_group(
        subscriber: Subscriber,
        subscription: Subscription,
        largest_group: u64,
    ) -> Self {
        let (object_sender, object_receiver) = mpsc::channel(OBJECT_CHANNEL_CAPACITY);
        let mut live = Self::of(
            Subscriber {
                session: subscriber.session.clone(),
            },
            subscription.clone(),
        );
        let join_handle = executor::spawn("Track Reader (new group)", async move {
            let mut subscriber = subscriber;
            let mut held = Vec::new();
            let mut waiting = true;
            let wait = executor::sleep(NEW_GROUP_WAIT);
            tokio::pin!(wait);
            loop {
                let object = tokio::select! {
                    object = live.next_event() => match object {
                        Ok(Some(TrackEvent::Object(object))) if waiting && object.group_id <= largest_group => {
                            held.push(TrackEvent::Object(object));
                            continue;
                        }
                        Ok(Some(TrackEvent::Object(object))) => {
                            waiting = false;
                            Ok(TrackEvent::Object(object))
                        }
                        Ok(Some(end @ TrackEvent::EndOfGroup { location, .. })) if waiting && location.group_id <= largest_group => {
                            held.push(end);
                            continue;
                        }
                        Ok(Some(event)) => Ok(event),
                        Ok(None) => return,
                        Err(error) => Err(error),
                    },
                    () = &mut wait, if waiting => {
                        waiting = false;
                        tracing::debug!(largest_group, "no new group arrived; fetching the current group");
                        let fetch = subscriber
                            .fetch_relative_joining(&subscription, 0, FetchOption::default())
                            .await;
                        send_fetched_group(&mut subscriber, fetch, object_sender.clone()).await;
                        for object in std::mem::take(&mut held) {
                            if object_sender.send(Ok(object)).await.is_err() {
                                return;
                            }
                        }
                        continue;
                    }
                };
                let failed = object.is_err();
                if object_sender.send(object).await.is_err() || failed {
                    return;
                }
            }
        })
        .abort_on_drop();
        Self {
            object_receiver,
            _track_read_task: TrackReadTask {
                _join_handle: join_handle,
            },
        }
    }

    /// Returns `Ok(None)` once the track has no more data. A failed subgroup
    /// stream only ends that stream. The only `Err` is a
    /// `MalformedTrackError`, which ends the track after the streams already
    /// accepted are drained.
    pub async fn next_object(&mut self) -> anyhow::Result<Option<TrackObject>> {
        loop {
            match self.next_event().await? {
                Some(TrackEvent::Object(object)) => return Ok(Some(object)),
                Some(TrackEvent::Reset { .. } | TrackEvent::EndOfGroup { .. }) => continue,
                None => return Ok(None),
            }
        }
    }

    /// `next_object`, with the subgroup streams the publisher reset and the
    /// End of Group status objects, each in its place among the objects of
    /// its subgroup.
    pub async fn next_event(&mut self) -> anyhow::Result<Option<TrackEvent>> {
        self.object_receiver.recv().await.transpose()
    }
}

/// Sends the objects of the answered Joining FETCH `fetch` to `object_sender`. For a track
/// without content the peer refuses the FETCH with INVALID_RANGE (draft-14
/// §9.16.2), which leaves the reader with the live objects only.
async fn send_fetched_group(
    subscriber: &mut Subscriber,
    fetch: Result<FetchHandle, RequestFailure>,
    object_sender: mpsc::Sender<anyhow::Result<TrackEvent>>,
) {
    let sent = async {
        let mut receiver = subscriber.accept_fetch_receiver(&fetch?).await?;
        loop {
            match receiver.receive().await? {
                Fetch::Object(object) => {
                    if let Some(object) = TrackEvent::from_object(object)
                        && object_sender.send(Ok(object)).await.is_err()
                    {
                        break;
                    }
                }
                Fetch::UnknownRange { .. } => {}
                Fetch::End => break,
            }
        }
        anyhow::Ok(())
    };
    if let Err(error) = sent.await {
        tracing::debug!(%error, "joining FETCH of the current group failed");
    }
}

struct TrackReadTask {
    _join_handle: AbortOnDrop,
}

impl TrackReadTask {
    fn run(
        receiver: impl Future<Output = anyhow::Result<DataReceiver>> + MaybeSend + 'static,
        object_sender: mpsc::Sender<anyhow::Result<TrackEvent>>,
    ) -> Self {
        let join_handle = executor::spawn("Track Reader", async move {
            let mut receiver = match receiver.await {
                Ok(receiver) => receiver,
                Err(error) => {
                    if error.is::<MalformedTrackError>() {
                        let _ = object_sender.send(Err(error)).await;
                    } else {
                        tracing::debug!(%error, "track has no data receiver");
                    }
                    return;
                }
            };
            let mut subgroup_readers = FuturesUnordered::new();
            loop {
                tokio::select! {
                    data = receiver.next() => match data {
                        Ok(TrackData::Subgroup(stream)) => {
                            subgroup_readers.push(read_subgroup(stream, object_sender.clone()));
                        }
                        Ok(TrackData::Datagram(object)) => {
                            if let Some(object) = TrackEvent::from_object(object)
                                && object_sender.send(Ok(object)).await.is_err()
                            {
                                return;
                            }
                        }
                        Err(error) if error.is::<MalformedTrackError>() => {
                            let _ = object_sender.send(Err(error)).await;
                            break;
                        }
                        Err(error) => {
                            tracing::debug!(%error, "track ended");
                            break;
                        }
                    },
                    Some(()) = subgroup_readers.next() => {}
                }
            }
            drop(object_sender);
            while subgroup_readers.next().await.is_some() {}
        })
        .abort_on_drop();
        Self {
            _join_handle: join_handle,
        }
    }
}

async fn read_subgroup(
    mut receiver: StreamDataReceiver,
    object_sender: mpsc::Sender<anyhow::Result<TrackEvent>>,
) {
    match read_subgroup_objects(&mut receiver, &object_sender).await {
        Ok(()) => {}
        Err(StreamReceiveError::Closed(ReadError::Reset(code))) => {
            let reset = TrackEvent::Reset {
                group_id: receiver.group_id(),
                code: receiver.version().data_stream_reset_code_from_wire(code),
            };
            let _ = object_sender.send(Ok(reset)).await;
        }
        Err(error) => tracing::debug!(%error, "subgroup stream failed; the track goes on"),
    }
}

async fn read_subgroup_objects(
    receiver: &mut StreamDataReceiver,
    object_sender: &mpsc::Sender<anyhow::Result<TrackEvent>>,
) -> Result<(), StreamReceiveError> {
    loop {
        match receiver.receive().await? {
            Some(Subgroup::Object(object)) => {
                let Some(object) = TrackEvent::from_object(object) else {
                    continue;
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, time::Duration};

    use bytes::Bytes;

    use crate::{
        TrackEvent, TrackObject, TrackReader,
        modules::moqt::data_plane::object::subgroup::{SubgroupId, SubgroupObject},
        modules::test_support::{
            datagram_object, open_raw_subgroup, open_subgroup, published_track, send_payload,
            send_raw_payload, subscribed_track_reader, within,
        },
        object::{
            DataStreamResetCode, ForwardingPreference, Location, MalformedTrackError, ObjectStatus,
        },
    };

    const NO_OBJECT_TIMEOUT: Duration = Duration::from_millis(500);

    async fn collect_objects(reader: &mut TrackReader, count: usize) -> Vec<TrackObject> {
        let mut objects = Vec::new();
        while objects.len() < count {
            let object = within(reader.next_object())
                .await
                .unwrap()
                .expect("track still open");
            objects.push(object);
        }
        objects
    }

    /// Group, subgroup (`None` for a datagram), object and payload.
    fn locations(objects: &[TrackObject]) -> BTreeSet<(u64, Option<u64>, u64, Bytes)> {
        objects
            .iter()
            .map(|object| {
                let subgroup_id = match object.forwarding {
                    ForwardingPreference::Subgroup { subgroup_id } => Some(subgroup_id),
                    ForwardingPreference::Datagram => None,
                };
                (
                    object.group_id,
                    subgroup_id,
                    object.object_id,
                    object.payload.clone(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn next_group_is_read_while_the_previous_stream_is_still_open() {
        // Arrange
        let track = published_track("reader-unclosed").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut group_0 = open_raw_subgroup(&factory, 0, SubgroupId::None).await;
        send_raw_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;

        // Act
        let mut group_1 = open_raw_subgroup(&factory, 1, SubgroupId::None).await;
        send_raw_payload(&mut group_1, b"g1-o0").await;
        send_raw_payload(&mut group_0, b"g0-o1").await;
        let objects = collect_objects(&mut reader, 3).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (0, Some(0), 0, Bytes::from_static(b"g0-o0")),
                (0, Some(0), 1, Bytes::from_static(b"g0-o1")),
                (1, Some(0), 0, Bytes::from_static(b"g1-o0")),
            ])
        );
    }

    #[tokio::test]
    async fn a_reader_made_before_the_first_stream_can_be_polled_and_dropped() {
        // Arrange
        let track = published_track("reader-before-first-stream").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut reader = track.server.subscriber().track_reader(&track.accepted);

        // Act: a poll that times out drops the future while nothing arrived
        let early = tokio::time::timeout(NO_OBJECT_TIMEOUT, reader.next_object()).await;
        let mut group_0 = open_raw_subgroup(&factory, 0, SubgroupId::None).await;
        send_raw_payload(&mut group_0, b"g0-o0").await;
        let objects = collect_objects(&mut reader, 1).await;

        // Assert
        assert!(early.is_err());
        assert_eq!(objects[0].payload, Bytes::from_static(b"g0-o0"));
    }

    #[tokio::test]
    async fn subgroups_of_one_group_are_read_concurrently() {
        // Arrange
        let track = published_track("reader-subgroups").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut subgroup_0 = open_raw_subgroup(&factory, 5, SubgroupId::Value(0)).await;
        send_raw_payload(&mut subgroup_0, b"s0-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;

        // Act
        let mut subgroup_1 = open_raw_subgroup(&factory, 5, SubgroupId::Value(1)).await;
        subgroup_1
            .send_raw(2, SubgroupObject::new_payload(Bytes::from_static(b"s1-o2")))
            .await
            .unwrap();
        send_raw_payload(&mut subgroup_0, b"s0-o1").await;
        let objects = collect_objects(&mut reader, 3).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (5, Some(0), 0, Bytes::from_static(b"s0-o0")),
                (5, Some(0), 1, Bytes::from_static(b"s0-o1")),
                (5, Some(1), 2, Bytes::from_static(b"s1-o2")),
            ])
        );
    }

    #[tokio::test]
    async fn first_object_id_subgroup_type_takes_the_first_object_id() {
        // Arrange
        let track = published_track("reader-first-id").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut subgroup = open_raw_subgroup(&factory, 2, SubgroupId::FirstObjectIdDelta).await;
        subgroup
            .send_raw(7, SubgroupObject::new_payload(Bytes::from_static(b"o7")))
            .await
            .unwrap();
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;

        // Act
        send_raw_payload(&mut subgroup, b"o8").await;
        let objects = collect_objects(&mut reader, 2).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([
                (2, Some(7), 7, Bytes::from_static(b"o7")),
                (2, Some(7), 8, Bytes::from_static(b"o8")),
            ])
        );
    }

    #[tokio::test]
    async fn status_objects_are_skipped_and_closing_a_stream_keeps_the_track_open() {
        // Arrange
        let track = published_track("reader-status").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut group_0 = open_raw_subgroup(&factory, 0, SubgroupId::None).await;
        send_raw_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;

        // Act
        group_0
            .send_raw(
                0,
                SubgroupObject::new_status(crate::object::ObjectStatus::EndOfGroup as u64),
            )
            .await
            .unwrap();
        group_0.close().await.unwrap();
        let objects = collect_objects(&mut reader, 1).await;
        let nothing_more = tokio::time::timeout(NO_OBJECT_TIMEOUT, reader.next_object()).await;

        // Assert
        assert_eq!(objects[0].payload, Bytes::from_static(b"g0-o0"));
        assert!(nothing_more.is_err(), "reader must wait for further groups");
    }

    #[tokio::test]
    async fn next_event_yields_the_end_of_group_after_the_objects_of_its_subgroup() {
        // Arrange
        let track = published_track("reader-end-of-group").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut group_0 = open_raw_subgroup(&factory, 4, SubgroupId::None).await;
        send_raw_payload(&mut group_0, b"g4-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;

        // Act
        group_0
            .send_raw(
                0,
                SubgroupObject::new_status(ObjectStatus::EndOfGroup as u64),
            )
            .await
            .unwrap();
        let object = within(reader.next_event()).await.unwrap();
        let end_of_group = within(reader.next_event()).await.unwrap();

        // Assert
        assert!(matches!(
            object,
            Some(TrackEvent::Object(TrackObject {
                group_id: 4,
                object_id: 0,
                ..
            }))
        ));
        assert!(matches!(
            end_of_group,
            Some(TrackEvent::EndOfGroup {
                location: Location {
                    group_id: 4,
                    object_id: 1
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            })
        ));
    }

    #[tokio::test]
    async fn objects_of_a_datagram_track_are_returned() {
        // Arrange
        let track = published_track("reader-datagrams").await;
        let mut sender = track.client.publisher().create_datagram(&track.published);
        let mut reader = track.server.subscriber().track_reader(&track.accepted);

        // Act
        sender.send_object(&datagram_object()).await.unwrap();
        let objects = collect_objects(&mut reader, 1).await;

        // Assert
        assert_eq!(
            locations(&objects),
            BTreeSet::from([(0, None, 0, Bytes::from_static(b"object"))])
        );
    }

    #[tokio::test]
    async fn a_reset_subgroup_stream_is_skipped_and_the_next_group_is_read() {
        // Arrange
        let track = published_track("reader-reset").await;
        let factory = track.client.publisher().create_stream(&track.published);
        let mut group_0 = open_subgroup(&factory, 0).await;
        send_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;
        collect_objects(&mut reader, 1).await;

        // Act
        group_0
            .sender
            .reset(DataStreamResetCode::Cancelled)
            .await
            .unwrap();
        let mut group_1 = open_subgroup(&factory, 1).await;
        send_payload(&mut group_1, b"g1-o0").await;
        let next = within(reader.next_object()).await;

        // Assert
        assert_eq!(next.unwrap().unwrap().payload, Bytes::from_static(b"g1-o0"));
    }

    #[tokio::test]
    async fn a_draft14_datagram_on_a_subgroup_track_is_returned_as_a_malformed_track_error() {
        // Arrange
        let track = published_track("reader-malformed").await;
        let publisher = track.client.publisher();
        let mut group_0 = open_raw_subgroup(
            &publisher.create_stream(&track.published),
            0,
            SubgroupId::None,
        )
        .await;
        send_raw_payload(&mut group_0, b"g0-o0").await;
        let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;
        collect_objects(&mut reader, 1).await;

        // Act
        publisher
            .create_datagram(&track.published)
            .send_object(&datagram_object())
            .await
            .unwrap();
        let result = within(reader.next_object()).await;

        // Assert
        assert_eq!(
            result.unwrap_err().downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::MixedForwardingPreference)
        );
    }
}

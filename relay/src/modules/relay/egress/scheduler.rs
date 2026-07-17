use std::{collections::HashSet, sync::Arc};

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::modules::{
    enums::{FilterType, GroupOrder},
    relay::{
        cache::track_cache::TrackCache, notifications::track_event::TrackEvent,
        types::StreamSubgroupId,
    },
};

fn resolve_start_location(
    filter_type: &FilterType,
    largest: &Option<moqt::Location>,
) -> moqt::Location {
    match (filter_type, largest) {
        (
            FilterType::AbsoluteStart { location } | FilterType::AbsoluteRange { location, .. },
            largest,
        ) => {
            let requested = location.as_moqt();
            match largest {
                Some(largest)
                    if (requested.group_id, requested.object_id)
                        <= (largest.group_id, largest.object_id) =>
                {
                    moqt::Location {
                        group_id: largest.group_id,
                        object_id: largest.object_id + 1,
                    }
                }
                _ => requested,
            }
        }
        // Largest Object (0x2): Start = {Largest.Group, Largest.Object + 1}.
        (FilterType::LargestObject, Some(largest)) => moqt::Location {
            group_id: largest.group_id,
            object_id: largest.object_id + 1,
        },
        // Next Group Start (0x1): Start = {Largest.Group + 1, 0}.
        (FilterType::NextGroupStart, Some(largest)) => moqt::Location {
            group_id: largest.group_id + 1,
            object_id: 0,
        },
        // No content delivered yet: Start = {0, 0}.
        (FilterType::LargestObject | FilterType::NextGroupStart, None) => moqt::Location {
            group_id: 0,
            object_id: 0,
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GroupSendTaskKey {
    Stream {
        group_id: u64,
        subgroup_id: StreamSubgroupId,
    },
    Datagram {
        group_id: u64,
    },
}

/// Instruction for `GroupSender` to transmit a stream subgroup or datagram
/// group from `object_id` on. Translating object ids to cache indices
/// is `GroupSender`'s concern.
pub(crate) enum GroupSendTask {
    Stream {
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        object_id: u64,
    },
    Datagram {
        group_id: u64,
        object_id: u64,
    },
}

impl GroupSendTask {
    fn key(&self) -> GroupSendTaskKey {
        match self {
            Self::Stream {
                group_id,
                subgroup_id,
                ..
            } => GroupSendTaskKey::Stream {
                group_id: *group_id,
                subgroup_id: subgroup_id.clone(),
            },
            Self::Datagram { group_id, .. } => GroupSendTaskKey::Datagram {
                group_id: *group_id,
            },
        }
    }
}

struct StartLocationProgress {
    start_group_id: u64,
    start_object_id: Option<u64>,
}

impl StartLocationProgress {
    fn accept(&mut self, group_id: u64) -> Option<u64> {
        if group_id < self.start_group_id {
            return None;
        }
        if group_id == self.start_group_id {
            Some(self.start_object_id.take().unwrap_or(0))
        } else {
            Some(0)
        }
    }
}

/// Watches track events and decides which egress units to schedule and when.
pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<TrackEvent>,
    filter_type: FilterType,
    group_order: GroupOrder,
    sender: mpsc::Sender<GroupSendTask>,
    ready_sender: Option<oneshot::Sender<anyhow::Result<()>>>,
    /// Largest Object at SUBSCRIBE processing time; `None` when no content
    /// has been delivered yet.
    largest_location: Option<moqt::Location>,
}

impl EgressScheduler {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<TrackEvent>,
        filter_type: FilterType,
        group_order: GroupOrder,
        sender: mpsc::Sender<GroupSendTask>,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
        largest_location: Option<moqt::Location>,
    ) -> Self {
        Self {
            cache,
            latest_info_sender,
            filter_type,
            group_order,
            sender,
            ready_sender: Some(ready_sender),
            largest_location,
        }
    }

    pub(crate) async fn run(mut self) {
        let mut receiver = self.latest_info_sender.subscribe();
        let mut scheduled = HashSet::<GroupSendTaskKey>::new();

        let start = resolve_start_location(&self.filter_type, &self.largest_location);
        self.schedule_cached_objects(&start, &mut scheduled).await;
        let mut progress = StartLocationProgress {
            start_group_id: start.group_id,
            start_object_id: Some(start.object_id),
        };
        self.notify_ready(Ok(()));

        loop {
            match receiver.recv().await {
                Ok(TrackEvent::StreamOpened {
                    group_id,
                    subgroup_id,
                }) => {
                    if let Some(object_id) = progress.accept(group_id) {
                        let sent = self
                            .schedule_subgroup_objects(
                                group_id,
                                subgroup_id,
                                object_id,
                                &mut scheduled,
                            )
                            .await;
                        if sent.is_some() {
                            self.recover_lagged_groups(group_id, &mut scheduled).await;
                        }
                    }
                }
                Ok(TrackEvent::DatagramOpened { group_id }) => {
                    if let Some(object_id) = progress.accept(group_id) {
                        let sent = self
                            .schedule_datagrams(group_id, object_id, &mut scheduled)
                            .await;
                        if sent.is_some() {
                            self.recover_lagged_groups(group_id, &mut scheduled).await;
                        }
                    }
                }
                Ok(TrackEvent::EndOfGroup) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress scheduler receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    fn notify_ready(&mut self, result: anyhow::Result<()>) {
        if let Some(sender) = self.ready_sender.take() {
            let _ = sender.send(result);
        }
    }

    /// Re-schedules consecutive cached groups after `group_id`, recovering
    /// groups whose open events were lost to receiver lag. Duplicates are
    /// filtered by the `scheduled` set.
    async fn recover_lagged_groups(
        &self,
        group_id: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) {
        if matches!(self.group_order, GroupOrder::Descending) {
            return;
        }
        self.schedule_cached_objects(
            &moqt::Location {
                group_id: group_id + 1,
                object_id: 0,
            },
            scheduled,
        )
        .await;
    }

    /// Schedules consecutive cached groups starting at the filter Start
    /// Location.
    ///
    /// With starts clamped to the subscribe-time Largest Object this never
    /// replays the past; what it covers is delivery that events cannot:
    /// the rest of the group already open at the Start Location (its open
    /// event predates this scheduler), groups arriving between the
    /// subscribe-time snapshot and event subscription, and lag recovery.
    async fn schedule_cached_objects(
        &self,
        start: &moqt::Location,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) {
        let mut next = start.group_id;
        while self.cache.has_stream_group(next).await || self.cache.has_datagram_group(next).await {
            let object_id = if next == start.group_id {
                start.object_id
            } else {
                0
            };
            for subgroup_id in self.cache.stream_subgroups(next).await {
                let _ = self
                    .schedule_subgroup_objects(next, subgroup_id, object_id, scheduled)
                    .await;
            }
            if self.cache.has_datagram_group(next).await {
                let _ = self.schedule_datagrams(next, object_id, scheduled).await;
            }
            if matches!(self.group_order, GroupOrder::Descending) {
                return;
            }
            next += 1;
        }
    }

    /// Schedules delivery of one stream subgroup starting at `object_id`.
    /// Returns `None` when the task channel is closed.
    async fn schedule_subgroup_objects(
        &self,
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        object_id: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<()> {
        let task = GroupSendTask::Stream {
            group_id,
            subgroup_id,
            object_id,
        };
        if !scheduled.insert(task.key()) {
            return Some(());
        }
        self.sender.send(task).await.ok()?;
        Some(())
    }

    /// Schedules delivery of one datagram group starting at `object_id`.
    /// Returns `None` when the task channel is closed.
    async fn schedule_datagrams(
        &self,
        group_id: u64,
        object_id: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<()> {
        let task = GroupSendTask::Datagram {
            group_id,
            object_id,
        };
        if !scheduled.insert(task.key()) {
            return Some(());
        }
        self.sender.send(task).await.ok()?;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::data_object::DataObject;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};

    fn make_header() -> DataObject {
        DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            0,
            SubgroupId::Value(0),
            0,
            false,
            false,
        ))
    }

    fn make_object(delta: u64) -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: delta,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from(vec![])),
        })
    }

    // Resolves the object_id the way the ingest stream reader does (per-stream prev),
    // then appends. A SubgroupHeader resolves to None and resets the chain.
    async fn append_stream(
        cache: &TrackCache,
        group_id: u64,
        subgroup: &StreamSubgroupId,
        prev_object_id: &mut Option<u64>,
        object: DataObject,
    ) {
        let object_id = object.resolve_absolute_object_id(*prev_object_id);
        *prev_object_id = object_id;
        cache
            .append_stream_object(group_id, subgroup, object_id, object)
            .await;
    }

    // Largest Object (0x2) filter must start delivery just after the Largest
    // Object (§9.7: Start = {Largest.Group, Largest.Object + 1}), not include it.
    #[tokio::test]
    async fn largest_object_filter_starts_after_largest_for_stream() {
        let cache = Arc::new(TrackCache::new());
        let subgroup = StreamSubgroupId::Value(0);
        // index 0: subgroup header, index 1: the Largest Object
        let mut prev = None;
        append_stream(&cache, 0, &subgroup, &mut prev, make_header()).await;
        append_stream(&cache, 0, &subgroup, &mut prev, make_object(0)).await;

        let (info_tx, _info_rx) = broadcast::channel(16);
        let (task_tx, mut task_rx) = mpsc::channel(16);
        let (ready_tx, _ready_rx) = tokio::sync::oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            info_tx,
            FilterType::LargestObject,
            GroupOrder::Ascending,
            task_tx,
            ready_tx,
            // What subscribe-time resolution against this cache would yield:
            // the Largest Object is {0, 0}.
            Some(moqt::Location {
                group_id: 0,
                object_id: 0,
            }),
        );
        let handle = tokio::spawn(scheduler.run());

        let task = task_rx.recv().await.expect("a task should be scheduled");
        match task {
            GroupSendTask::Stream {
                group_id,
                object_id,
                ..
            } => {
                assert_eq!(group_id, 0);
                // Largest Object is at cache index 1, so delivery starts at index 2.
                assert_eq!(object_id, 1);
            }
            _ => panic!("expected a Stream task"),
        }
        handle.abort();
    }

    // Subscriptions only deliver newly published or received objects;
    // objects from the past are retrieved with FETCH (§9.7). An
    // AbsoluteStart in the past is therefore raised to just after the
    // subscribe-time Largest Object instead of replaying the cache.
    #[tokio::test]
    async fn absolute_start_in_the_past_does_not_replay_cache() {
        let cache = Arc::new(TrackCache::new());
        let subgroup = StreamSubgroupId::Value(0);
        for group_id in 0..3 {
            let mut prev = None;
            append_stream(&cache, group_id, &subgroup, &mut prev, make_header()).await;
            append_stream(&cache, group_id, &subgroup, &mut prev, make_object(0)).await;
        }

        let (info_tx, _info_rx) = broadcast::channel(16);
        let (task_tx, mut task_rx) = mpsc::channel(16);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            info_tx,
            FilterType::AbsoluteStart {
                location: crate::modules::enums::Location {
                    group_id: 0,
                    object_id: 0,
                },
            },
            GroupOrder::Ascending,
            task_tx,
            ready_tx,
            // Largest Object at subscribe time: group 2, object 0.
            Some(moqt::Location {
                group_id: 2,
                object_id: 0,
            }),
        );
        let handle = tokio::spawn(scheduler.run());
        ready_rx
            .await
            .expect("scheduler should signal readiness")
            .expect("scheduler should start");

        // Only the tail of the largest group is scheduled; groups 0 and 1
        // stay in the cache for FETCH.
        let task = task_rx.recv().await.expect("a task should be scheduled");
        match task {
            GroupSendTask::Stream {
                group_id,
                object_id,
                ..
            } => {
                assert_eq!(group_id, 2);
                assert_eq!(object_id, 1);
            }
            _ => panic!("expected a Stream task"),
        }
        assert!(
            task_rx.try_recv().is_err(),
            "past groups must not be scheduled"
        );
        handle.abort();
    }

    // The Start Location is a lower bound (§9.7): with no content yet the
    // start is {0, 0}, and delivery must begin from whatever group arrives
    // first — group ids may start anywhere (§2.3.1), so waiting for group 0
    // exactly would stall forever.
    #[tokio::test]
    async fn start_location_is_lower_bound_for_first_arriving_group() {
        let cache = Arc::new(TrackCache::new());
        let (info_tx, _info_rx) = broadcast::channel(16);
        let (task_tx, mut task_rx) = mpsc::channel(16);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            info_tx.clone(),
            FilterType::LargestObject,
            GroupOrder::Ascending,
            task_tx,
            ready_tx,
            None,
        );
        let handle = tokio::spawn(scheduler.run());
        ready_rx
            .await
            .expect("scheduler should signal readiness")
            .expect("scheduler should start");

        info_tx
            .send(TrackEvent::StreamOpened {
                group_id: 5,
                subgroup_id: StreamSubgroupId::Value(0),
            })
            .expect("event should reach the scheduler");

        let task = task_rx.recv().await.expect("a task should be scheduled");
        match task {
            GroupSendTask::Stream { group_id, .. } => assert_eq!(group_id, 5),
            _ => panic!("expected a Stream task"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn new_upstream_largest_object_without_content_starts_from_first_object() {
        let cache = Arc::new(TrackCache::new());
        let subgroup = StreamSubgroupId::Value(0);
        let mut prev = None;
        append_stream(&cache, 0, &subgroup, &mut prev, make_header()).await;
        append_stream(&cache, 0, &subgroup, &mut prev, make_object(0)).await;

        let (info_tx, _info_rx) = broadcast::channel(16);
        let (task_tx, mut task_rx) = mpsc::channel(16);
        let (ready_tx, _ready_rx) = tokio::sync::oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            info_tx,
            FilterType::LargestObject,
            GroupOrder::Ascending,
            task_tx,
            ready_tx,
            None,
        );
        let handle = tokio::spawn(scheduler.run());

        let task = task_rx.recv().await.expect("a task should be scheduled");
        match task {
            GroupSendTask::Stream {
                group_id,
                object_id,
                ..
            } => {
                assert_eq!(group_id, 0);
                assert_eq!(object_id, 0);
            }
            _ => panic!("expected a Stream task"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn new_upstream_largest_object_with_content_starts_after_subscribe_ok_location() {
        let cache = Arc::new(TrackCache::new());
        let subgroup = StreamSubgroupId::Value(0);
        let mut prev = None;
        append_stream(&cache, 0, &subgroup, &mut prev, make_header()).await;
        append_stream(&cache, 0, &subgroup, &mut prev, make_object(0)).await;
        append_stream(&cache, 0, &subgroup, &mut prev, make_object(1)).await;

        let (info_tx, _info_rx) = broadcast::channel(16);
        let (task_tx, mut task_rx) = mpsc::channel(16);
        let (ready_tx, _ready_rx) = tokio::sync::oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            info_tx,
            FilterType::LargestObject,
            GroupOrder::Ascending,
            task_tx,
            ready_tx,
            Some(moqt::Location {
                group_id: 0,
                object_id: 0,
            }),
        );
        let handle = tokio::spawn(scheduler.run());

        let task = task_rx.recv().await.expect("a task should be scheduled");
        match task {
            GroupSendTask::Stream {
                group_id,
                object_id,
                ..
            } => {
                assert_eq!(group_id, 0);
                assert_eq!(object_id, 1);
            }
            _ => panic!("expected a Stream task"),
        }
        handle.abort();
    }
}

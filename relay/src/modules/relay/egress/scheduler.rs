use std::{collections::HashSet, sync::Arc};

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::modules::{
    enums::{FilterType, GroupOrder},
    relay::{
        cache::track_cache::TrackCache, notifications::track_event::TrackEvent, types::SubgroupKey,
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

/// Instruction for `GroupSender` to transmit one subgroup (or datagram group)
/// from `object_id` on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GroupSendTask {
    pub(crate) key: SubgroupKey,
    pub(crate) object_id: u64,
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
        let mut scheduled = HashSet::<SubgroupKey>::new();

        let start = resolve_start_location(&self.filter_type, &self.largest_location);
        self.schedule_cached_objects(&start, &mut scheduled).await;
        let mut progress = StartLocationProgress {
            start_group_id: start.group_id,
            start_object_id: Some(start.object_id),
        };
        self.notify_ready(Ok(()));

        loop {
            match receiver.recv().await {
                Ok(TrackEvent::SubgroupOpened(key)) => {
                    if let Some(object_id) = progress.accept(key.group_id())
                        && self
                            .schedule(key, object_id, &mut scheduled)
                            .await
                            .is_some()
                    {
                        self.recover_lagged_groups(key.group_id(), &mut scheduled)
                            .await;
                    }
                }
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
    async fn recover_lagged_groups(&self, group_id: u64, scheduled: &mut HashSet<SubgroupKey>) {
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
        scheduled: &mut HashSet<SubgroupKey>,
    ) {
        let mut next = start.group_id;
        while self.cache.has_group(next) {
            let object_id = if next == start.group_id {
                start.object_id
            } else {
                0
            };
            for key in self.cache.subgroups_in_group(next) {
                let _ = self.schedule(key, object_id, scheduled).await;
            }
            if matches!(self.group_order, GroupOrder::Descending) {
                return;
            }
            next += 1;
        }
    }

    /// Schedules delivery of one subgroup starting at `object_id`.
    /// Returns `None` when the task channel is closed.
    async fn schedule(
        &self,
        key: SubgroupKey,
        object_id: u64,
        scheduled: &mut HashSet<SubgroupKey>,
    ) -> Option<()> {
        if !scheduled.insert(key) {
            return Some(());
        }
        self.sender
            .send(GroupSendTask { key, object_id })
            .await
            .ok()?;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::tests::harness::fixtures::cached_object::{
        insert_closed_live_group, stream_key,
    };

    struct RunningScheduler {
        task_receiver: mpsc::Receiver<GroupSendTask>,
        event_sender: broadcast::Sender<TrackEvent>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Drop for RunningScheduler {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn start_scheduler(
        cache: Arc<TrackCache>,
        filter_type: FilterType,
        largest_location: Option<moqt::Location>,
    ) -> RunningScheduler {
        let (event_sender, _event_receiver) = broadcast::channel(16);
        let (task_sender, task_receiver) = mpsc::channel(16);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            event_sender.clone(),
            filter_type,
            GroupOrder::Ascending,
            task_sender,
            ready_sender,
            largest_location,
        );
        let handle = tokio::spawn(scheduler.run());
        ready_receiver
            .await
            .expect("scheduler should signal readiness")
            .expect("scheduler should start");
        RunningScheduler {
            task_receiver,
            event_sender,
            handle,
        }
    }

    fn location(group_id: u64, object_id: u64) -> moqt::Location {
        moqt::Location {
            group_id,
            object_id,
        }
    }

    // Largest Object (0x2) filter must start delivery just after the Largest
    // Object (§9.7: Start = {Largest.Group, Largest.Object + 1}), not include it.
    #[tokio::test]
    async fn largest_object_filter_starts_after_largest_for_stream() {
        // Arrange: object 0 of group 0 is the Largest Object at subscribe time
        let cache = Arc::new(TrackCache::new());
        insert_closed_live_group(&cache, 0, &[0]);
        // Act
        let mut scheduler =
            start_scheduler(cache, FilterType::LargestObject, Some(location(0, 0))).await;
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                key: stream_key(0),
                object_id: 1
            }
        );
    }

    // Subscriptions only deliver newly published or received objects;
    // objects from the past are retrieved with FETCH (§9.7). An
    // AbsoluteStart in the past is therefore raised to just after the
    // subscribe-time Largest Object instead of replaying the cache.
    #[tokio::test]
    async fn absolute_start_in_the_past_does_not_replay_cache() {
        // Arrange: groups 0..=2 are cached and group 2 holds the Largest Object
        let cache = Arc::new(TrackCache::new());
        for group_id in 0..3 {
            insert_closed_live_group(&cache, group_id, &[0]);
        }
        // Act
        let mut scheduler = start_scheduler(
            cache,
            FilterType::AbsoluteStart {
                location: crate::modules::enums::Location {
                    group_id: 0,
                    object_id: 0,
                },
            },
            Some(location(2, 0)),
        )
        .await;
        // Assert: only the tail of the largest group is scheduled; groups 0 and 1
        // stay in the cache for FETCH.
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                key: stream_key(2),
                object_id: 1
            }
        );
        assert!(
            scheduler.task_receiver.try_recv().is_err(),
            "past groups must not be scheduled"
        );
    }

    // The Start Location is a lower bound (§9.7): with no content yet the
    // start is {0, 0}, and delivery must begin from whatever group arrives
    // first — group ids may start anywhere (§2.3.1), so waiting for group 0
    // exactly would stall forever.
    #[tokio::test]
    async fn start_location_is_lower_bound_for_first_arriving_group() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let mut scheduler = start_scheduler(cache, FilterType::LargestObject, None).await;
        // Act
        scheduler
            .event_sender
            .send(TrackEvent::SubgroupOpened(stream_key(5)))
            .expect("event should reach the scheduler");
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(task.key, stream_key(5));
    }

    #[tokio::test]
    async fn new_upstream_largest_object_without_content_starts_from_first_object() {
        // Arrange: the cache already holds object 0, but SUBSCRIBE_OK reported no content
        let cache = Arc::new(TrackCache::new());
        insert_closed_live_group(&cache, 0, &[0]);
        // Act
        let mut scheduler = start_scheduler(cache, FilterType::LargestObject, None).await;
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                key: stream_key(0),
                object_id: 0
            }
        );
    }

    #[tokio::test]
    async fn new_upstream_largest_object_with_content_starts_after_subscribe_ok_location() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        insert_closed_live_group(&cache, 0, &[0, 1]);
        // Act
        let mut scheduler =
            start_scheduler(cache, FilterType::LargestObject, Some(location(0, 0))).await;
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                key: stream_key(0),
                object_id: 1
            }
        );
    }
}

use std::{collections::HashMap, sync::Arc};

use moqt::{FilterType, GroupOrder};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::modules::relay::{
    cache::track_cache::{SubgroupRun, TrackCache},
    types::SubgroupKey,
};

/// Subscriptions only deliver objects newer than the subscribe-time Largest
/// Object (§9.7), so an absolute start at or below it is raised past it.
fn resolve_start_location(
    filter_type: &FilterType,
    largest: &Option<moqt::Location>,
) -> moqt::Location {
    let start = filter_type.start_location(*largest);
    match largest {
        Some(largest) if start <= *largest => moqt::Location {
            group_id: largest.group_id,
            object_id: largest.object_id + 1,
        },
        _ => start,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroupSendTask {
    pub(crate) key: SubgroupKey,
    pub(crate) generation: usize,
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

pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    filter_type: FilterType,
    group_order: GroupOrder,
    sender: mpsc::Sender<GroupSendTask>,
    /// Largest Object at SUBSCRIBE processing time; `None` when no content
    /// has been delivered yet.
    largest_location: Option<moqt::Location>,
    forward_receiver: watch::Receiver<bool>,
}

impl EgressScheduler {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        filter_type: FilterType,
        group_order: GroupOrder,
        sender: mpsc::Sender<GroupSendTask>,
        largest_location: Option<moqt::Location>,
        forward_receiver: watch::Receiver<bool>,
    ) -> Self {
        Self {
            cache,
            filter_type,
            group_order,
            sender,
            largest_location,
            forward_receiver,
        }
    }

    pub(crate) async fn run(self, ready_sender: oneshot::Sender<anyhow::Result<()>>) {
        let mut receiver = self.cache.subscribe_subgroup_opened();
        let mut scheduled = HashMap::<SubgroupKey, GroupSendTask>::new();

        let start = resolve_start_location(&self.filter_type, &self.largest_location);
        self.schedule_cached_objects(&start, &mut scheduled).await;
        let mut progress = StartLocationProgress {
            start_group_id: start.group_id,
            start_object_id: Some(start.object_id),
        };
        let _ = ready_sender.send(Ok(()));

        loop {
            match receiver.recv().await {
                Ok(_) if !*self.forward_receiver.borrow() => {}
                Ok(run) => {
                    let group_id = run.key.group_id();
                    if let Some(object_id) = progress.accept(group_id)
                        && self
                            .schedule(run, object_id, &mut scheduled)
                            .await
                            .is_some()
                    {
                        self.recover_lagged_groups(group_id, &mut scheduled).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress scheduler receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// Re-schedules cached groups after `group_id`, recovering groups whose
    /// open events were lost to receiver lag. Duplicates are filtered by the
    /// `scheduled` map.
    async fn recover_lagged_groups(
        &self,
        group_id: u64,
        scheduled: &mut HashMap<SubgroupKey, GroupSendTask>,
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

    /// Schedules every cached group at or after the filter Start Location.
    ///
    /// The Start Location is a lower bound (§9.7), not a group that has to
    /// exist: group ids may start anywhere and skip values (§2.3.1), so the
    /// first cached group can lie above the start group.
    ///
    /// With starts clamped to the subscribe-time Largest Object this never
    /// replays the past; what it covers is delivery that events cannot:
    /// the rest of the group already open at the Start Location (its open
    /// event predates this scheduler), groups arriving between the
    /// subscribe-time snapshot and event subscription, and lag recovery.
    async fn schedule_cached_objects(
        &self,
        start: &moqt::Location,
        scheduled: &mut HashMap<SubgroupKey, GroupSendTask>,
    ) {
        for group_id in self.cache.groups_at_or_after(start.group_id) {
            let object_id = if group_id == start.group_id {
                start.object_id
            } else {
                0
            };
            for run in self.cache.subgroup_runs_in_group(group_id) {
                let _ = self.schedule(run, object_id, scheduled).await;
            }
            if matches!(self.group_order, GroupOrder::Descending) {
                return;
            }
        }
    }

    /// A reopened run resumes from where the key's previous task started, so the
    /// subscription's start clamp still holds, but never below the run itself.
    async fn schedule(
        &self,
        run: SubgroupRun,
        object_id: u64,
        scheduled: &mut HashMap<SubgroupKey, GroupSendTask>,
    ) -> Option<()> {
        let object_id = match scheduled.get(&run.key) {
            Some(previous) if previous.generation >= run.generation => return Some(()),
            Some(previous) => previous.object_id,
            None => object_id,
        };
        let task = GroupSendTask {
            key: run.key,
            generation: run.generation,
            object_id: object_id.max(run.first_object_id),
        };
        scheduled.insert(run.key, task);
        self.sender.send(task).await.ok()?;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::tests::harness::fixtures::{
        cached_object::{insert_aborted_group, insert_closed_group, stream_key},
        location,
    };

    struct RunningScheduler {
        task_receiver: mpsc::Receiver<GroupSendTask>,
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
        let (task_sender, task_receiver) = mpsc::channel(16);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let scheduler = EgressScheduler::new(
            cache,
            filter_type,
            GroupOrder::Ascending,
            task_sender,
            largest_location,
            watch::channel(true).1,
        );
        let handle = tokio::spawn(scheduler.run(ready_sender));
        ready_receiver
            .await
            .expect("scheduler should signal readiness")
            .expect("scheduler should start");
        RunningScheduler {
            task_receiver,
            handle,
        }
    }

    // Largest Object (0x2) filter must start delivery just after the Largest
    // Object (§9.7: Start = {Largest.Group, Largest.Object + 1}), not include it.
    #[tokio::test]
    async fn largest_object_filter_starts_after_largest_for_stream() {
        // Arrange: object 0 of group 0 is the Largest Object at subscribe time
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 0, &[0]);
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
                generation: 0,
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
            insert_closed_group(&cache, group_id, &[0]);
        }
        // Act
        let mut scheduler = start_scheduler(
            cache,
            FilterType::AbsoluteStart {
                location: moqt::Location {
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
                generation: 0,
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
        let mut scheduler = start_scheduler(cache.clone(), FilterType::LargestObject, None).await;
        // Act
        let _open = cache.open_subgroup(stream_key(5));
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(task.key, stream_key(5));
    }

    // The cached counterpart of the event case above: a first group that was
    // ingested before the scheduler subscribed to open events is only
    // reachable through the cache, and its id need not be the start group.
    #[tokio::test]
    async fn start_location_is_lower_bound_for_first_cached_group() {
        // Arrange: no content at subscribe time, group 5 already cached and closed
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 5, &[0]);
        // Act
        let mut scheduler = start_scheduler(cache, FilterType::NextGroupStart, None).await;
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                generation: 0,
                key: stream_key(5),
                object_id: 0
            }
        );
    }

    #[tokio::test]
    async fn next_group_start_schedules_cached_groups_across_a_group_id_gap() {
        // Arrange: group 3 holds the Largest Object; the publisher skipped to group 7
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 3, &[0]);
        insert_closed_group(&cache, 7, &[0]);
        // Act
        let mut scheduler =
            start_scheduler(cache, FilterType::NextGroupStart, Some(location(3, 0))).await;
        // Assert
        let task = scheduler
            .task_receiver
            .recv()
            .await
            .expect("a task should be scheduled");
        assert_eq!(
            task,
            GroupSendTask {
                generation: 0,
                key: stream_key(7),
                object_id: 0
            }
        );
    }

    #[tokio::test]
    async fn new_upstream_largest_object_without_content_starts_from_first_object() {
        // Arrange: the cache already holds object 0, but SUBSCRIBE_OK reported no content
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 0, &[0]);
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
                generation: 0,
                key: stream_key(0),
                object_id: 0
            }
        );
    }

    #[tokio::test]
    async fn new_upstream_largest_object_with_content_starts_after_subscribe_ok_location() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 0, &[0, 1]);
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
                generation: 0,
                key: stream_key(0),
                object_id: 1
            }
        );
    }

    #[tokio::test]
    async fn reopened_subgroup_is_scheduled_again_for_its_new_run() {
        // Arrange: objects 0..=2 were cached before the upstream stream was reset
        let cache = Arc::new(TrackCache::new());
        insert_aborted_group(&cache, 0, &[0, 1, 2]);
        let mut scheduler = start_scheduler(
            cache.clone(),
            FilterType::LargestObject,
            Some(location(0, 2)),
        )
        .await;
        let aborted_run_task = scheduler.task_receiver.recv().await;
        // Act
        let _reopened = cache.open_subgroup(stream_key(0));
        let _duplicate = cache.open_subgroup(stream_key(0));
        let _next_group = cache.open_subgroup(stream_key(1));
        // Assert: the duplicate open of the same run is skipped
        let tasks = [
            aborted_run_task,
            scheduler.task_receiver.recv().await,
            scheduler.task_receiver.recv().await,
        ];
        assert_eq!(
            tasks.map(|task| task.map(|task| (task.key, task.generation, task.object_id))),
            [
                Some((stream_key(0), 0, 3)),
                Some((stream_key(0), 1, 3)),
                Some((stream_key(1), 0, 0)),
            ]
        );
    }
}

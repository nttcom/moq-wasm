use std::{collections::HashMap, sync::Arc};

use moqt::object::GroupOrder;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::modules::{
    data_plane::{
        cache::subgroup_key::SubgroupKey,
        cache::track_cache::{SubgroupRun, TrackCache},
    },
    domain::delivery_control::DeliveryControl,
};

/// Subscriptions only deliver objects newer than the subscribe-time Largest
/// Object (§9.7), so an absolute start at or below it is raised past it.
fn resolve_start_location(
    start: moqt::object::Location,
    largest: &Option<moqt::object::Location>,
) -> moqt::object::Location {
    match largest {
        Some(largest) if start <= *largest => largest.next(),
        _ => start,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchedulerExit {
    /// Every group of the subscription's range has been scheduled.
    EndGroupReached,
    OpenEventsClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroupSendTask {
    pub(crate) key: SubgroupKey,
    pub(crate) generation: usize,
    pub(crate) object_id: u64,
}

struct StartLocationProgress {
    start: moqt::object::Location,
    start_group_seen: bool,
}

impl StartLocationProgress {
    fn new(start: moqt::object::Location) -> Self {
        Self {
            start,
            start_group_seen: false,
        }
    }

    /// Moves the start to a later Start Location from a SUBSCRIBE_UPDATE;
    /// an earlier one leaves it unchanged.
    fn raise(&mut self, start: moqt::object::Location) {
        if start > self.start {
            *self = Self::new(start);
        }
    }

    fn accept(&mut self, group_id: u64) -> Option<u64> {
        if group_id < self.start.group_id {
            return None;
        }
        if group_id == self.start.group_id && !self.start_group_seen {
            self.start_group_seen = true;
            Some(self.start.object_id)
        } else {
            Some(0)
        }
    }
}

pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    group_order: GroupOrder,
    sender: mpsc::Sender<GroupSendTask>,
    /// Largest Object at SUBSCRIBE processing time; `None` when no content
    /// has been delivered yet.
    largest_location: Option<moqt::object::Location>,
    delivery_control_receiver: watch::Receiver<DeliveryControl>,
}

impl EgressScheduler {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        group_order: GroupOrder,
        sender: mpsc::Sender<GroupSendTask>,
        largest_location: Option<moqt::object::Location>,
        delivery_control_receiver: watch::Receiver<DeliveryControl>,
    ) -> Self {
        Self {
            cache,
            group_order,
            sender,
            largest_location,
            delivery_control_receiver,
        }
    }

    pub(crate) async fn run(
        self,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
    ) -> SchedulerExit {
        let mut receiver = self.cache.subscribe_subgroup_opened();
        let mut delivery_control_receiver = self.delivery_control_receiver.clone();
        let mut scheduled = HashMap::<SubgroupKey, GroupSendTask>::new();

        let control = *delivery_control_receiver.borrow_and_update();
        let start = resolve_start_location(control.start, &self.largest_location);
        if control.forward {
            self.schedule_cached_objects(&start, &control, &mut scheduled)
                .await;
        }
        let mut progress = StartLocationProgress::new(start);
        let _ = ready_sender.send(Ok(()));
        if self.range_ended(&progress, &control) {
            return SchedulerExit::EndGroupReached;
        }

        let mut delivery_control_open = true;
        loop {
            tokio::select! {
                event = receiver.recv() => match event {
                    Ok(run) => {
                        let control = *delivery_control_receiver.borrow();
                        let group_id = run.key.group_id();
                        if control.is_past_end(group_id) {
                            return SchedulerExit::EndGroupReached;
                        }
                        if !control.forward {
                            continue;
                        }
                        progress.raise(control.start);
                        if let Some(object_id) = progress.accept(group_id)
                            && self
                                .schedule(run, object_id, &mut scheduled)
                                .await
                                .is_some()
                        {
                            self.recover_lagged_groups(group_id, &control, &mut scheduled)
                                .await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "egress scheduler receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return SchedulerExit::OpenEventsClosed;
                    }
                },
                changed = delivery_control_receiver.changed(), if delivery_control_open => {
                    if changed.is_err() {
                        delivery_control_open = false;
                        continue;
                    }
                    let control = *delivery_control_receiver.borrow_and_update();
                    progress.raise(control.start);
                    if self.range_ended(&progress, &control) {
                        return SchedulerExit::EndGroupReached;
                    }
                }
            }
        }
    }

    /// Live groups arrive in ascending order, so once the cache has opened a
    /// group at or past the End Group no further group can enter the range;
    /// a start at or past the End Group leaves nothing to deliver at all.
    fn range_ended(&self, progress: &StartLocationProgress, control: &DeliveryControl) -> bool {
        let Some(end_group) = control.end_group else {
            return false;
        };
        progress.start.group_id >= end_group || !self.cache.groups_at_or_after(end_group).is_empty()
    }

    /// Re-schedules cached groups after `group_id`, recovering groups whose
    /// open events were lost to receiver lag. Duplicates are filtered by the
    /// `scheduled` map.
    async fn recover_lagged_groups(
        &self,
        group_id: u64,
        control: &DeliveryControl,
        scheduled: &mut HashMap<SubgroupKey, GroupSendTask>,
    ) {
        if matches!(self.group_order, GroupOrder::Descending) {
            return;
        }
        self.schedule_cached_objects(
            &moqt::object::Location {
                group_id: group_id + 1,
                object_id: 0,
            },
            control,
            scheduled,
        )
        .await;
    }

    /// Schedules every cached group at or after the filter Start Location and
    /// before the End Group of `control`.
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
        start: &moqt::object::Location,
        control: &DeliveryControl,
        scheduled: &mut HashMap<SubgroupKey, GroupSendTask>,
    ) {
        for group_id in self.cache.groups_at_or_after(start.group_id) {
            if control.is_past_end(group_id) {
                return;
            }
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
    use moqt::subscribe::FilterType;

    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::{
        cached_object::{insert_aborted_group, insert_closed_group, stream_key},
        location,
    };

    const EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

    struct RunningScheduler {
        task_receiver: mpsc::Receiver<GroupSendTask>,
        delivery_control_sender: watch::Sender<DeliveryControl>,
        handle: tokio::task::JoinHandle<SchedulerExit>,
    }

    impl RunningScheduler {
        async fn exit(&mut self) -> SchedulerExit {
            tokio::time::timeout(EXIT_TIMEOUT, &mut self.handle)
                .await
                .expect("scheduler should exit")
                .expect("scheduler should not panic")
        }
    }

    impl Drop for RunningScheduler {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn start_scheduler(
        cache: Arc<TrackCache>,
        filter_type: FilterType,
        largest_location: Option<moqt::object::Location>,
    ) -> RunningScheduler {
        start_scheduler_with_forward(cache, filter_type, largest_location, true).await
    }

    async fn start_scheduler_with_forward(
        cache: Arc<TrackCache>,
        filter_type: FilterType,
        largest_location: Option<moqt::object::Location>,
        forward: bool,
    ) -> RunningScheduler {
        let (task_sender, task_receiver) = mpsc::channel(16);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (delivery_control_sender, delivery_control_receiver) = watch::channel(
            DeliveryControl::new(forward, &filter_type, largest_location),
        );
        let scheduler = EgressScheduler::new(
            cache,
            GroupOrder::Ascending,
            task_sender,
            largest_location,
            delivery_control_receiver,
        );
        let handle = tokio::spawn(scheduler.run(ready_sender));
        ready_receiver
            .await
            .expect("scheduler should signal readiness")
            .expect("scheduler should start");
        RunningScheduler {
            task_receiver,
            delivery_control_sender,
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

    #[tokio::test]
    async fn forward_zero_at_start_schedules_no_cached_group() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        insert_closed_group(&cache, 1, &[0]);

        // Act
        let mut scheduler =
            start_scheduler_with_forward(cache, FilterType::LargestObject, None, false).await;

        // Assert
        assert!(scheduler.task_receiver.try_recv().is_err());
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
                location: moqt::object::Location {
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
    async fn absolute_range_schedules_no_cached_group_past_its_end_group() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        for group_id in 1..4 {
            insert_closed_group(&cache, group_id, &[0]);
        }
        let filter_type = FilterType::AbsoluteRange {
            location: location(1, 0),
            end_group: 2,
        };

        // Act
        let mut scheduler = start_scheduler(cache, filter_type, None).await;

        // Assert
        let mut scheduled_groups = Vec::new();
        while let Ok(task) = scheduler.task_receiver.try_recv() {
            scheduled_groups.push(task.key.group_id());
        }
        assert_eq!(scheduled_groups, vec![1, 2]);
    }

    #[tokio::test]
    async fn opening_a_group_past_the_end_group_ends_the_schedule() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let filter_type = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 1,
        };
        let mut scheduler = start_scheduler(cache.clone(), filter_type, None).await;

        // Act
        let _open = cache.open_subgroup(stream_key(2));

        // Assert
        assert_eq!(scheduler.exit().await, SchedulerExit::EndGroupReached);
        assert!(scheduler.task_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn start_past_the_end_group_ends_the_schedule_at_once() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let filter_type = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 1,
        };

        // Act: the subscribe-time Largest Object raises the start to {3, 1}
        let mut scheduler = start_scheduler(cache, filter_type, Some(location(3, 0))).await;

        // Assert
        assert_eq!(scheduler.exit().await, SchedulerExit::EndGroupReached);
    }

    #[tokio::test]
    async fn update_moving_end_group_to_an_opened_group_ends_the_schedule() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let mut scheduler = start_scheduler(cache.clone(), FilterType::LargestObject, None).await;
        let _open = cache.open_subgroup(stream_key(3));
        let _ = scheduler.task_receiver.recv().await;

        // Act
        scheduler.delivery_control_sender.send_modify(|control| {
            control.apply_update(DeliveryControl::from_subscribe_update(
                true,
                location(0, 0),
                3,
            ));
        });

        // Assert
        assert_eq!(scheduler.exit().await, SchedulerExit::EndGroupReached);
    }

    #[tokio::test]
    async fn update_moving_end_group_past_the_open_groups_keeps_scheduling() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let mut scheduler = start_scheduler(cache.clone(), FilterType::LargestObject, None).await;
        let _open = cache.open_subgroup(stream_key(3));
        let _ = scheduler.task_receiver.recv().await;
        scheduler.delivery_control_sender.send_modify(|control| {
            control.apply_update(DeliveryControl::from_subscribe_update(
                true,
                location(0, 0),
                5,
            ));
        });

        // Act
        let _next = cache.open_subgroup(stream_key(4));

        // Assert
        let task = scheduler.task_receiver.recv().await;
        assert_eq!(task.map(|task| task.key), Some(stream_key(4)));
        assert!(!scheduler.handle.is_finished());
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

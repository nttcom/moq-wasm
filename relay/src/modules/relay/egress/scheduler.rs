use std::{collections::HashSet, sync::Arc};

use tokio::sync::{broadcast, mpsc};

use crate::modules::{
    enums::{FilterType, GroupOrder},
    relay::{
        cache::track_cache::TrackCache,
        notifications::track_event::TrackEvent,
        types::{CacheLocation, StreamSubgroupId},
    },
};

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

/// Instruction sent to `GroupSender` to transmit a stream subgroup or datagram group.
pub(crate) enum GroupSendTask {
    Stream {
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        start_offset: u64,
    },
    Datagram {
        group_id: u64,
        start_offset: u64,
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

/// Watches track events and decides which egress units to schedule and when.
pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<TrackEvent>,
    filter_type: FilterType,
    group_order: GroupOrder,
    sender: mpsc::Sender<GroupSendTask>,
}

impl EgressScheduler {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<TrackEvent>,
        filter_type: FilterType,
        group_order: GroupOrder,
        sender: mpsc::Sender<GroupSendTask>,
    ) -> Self {
        Self {
            cache,
            latest_info_sender,
            filter_type,
            group_order,
            sender,
        }
    }

    pub(crate) async fn run(self) {
        let mut receiver = self.latest_info_sender.subscribe();
        let mut scheduled = HashSet::<GroupSendTaskKey>::new();
        let next_group_start_floor = if matches!(self.filter_type, FilterType::NextGroupStart) {
            self.cache.latest_group_id().await
        } else {
            None
        };
        let absolute_start = Self::resolve_absolute_start(&self.filter_type);
        let mut next_absolute_group = absolute_start.map(|(group_id, _)| group_id);
        let absolute_group_id = absolute_start.map(|(group_id, _)| group_id);
        let mut start_offset_remaining = absolute_start.map(|(_, start_offset)| start_offset);

        if matches!(self.filter_type, FilterType::LargestObject)
            && let Some(location) = self.cache.latest_location().await
        {
            self.schedule_location(location, &mut scheduled).await;
        }

        loop {
            match receiver.recv().await {
                Ok(TrackEvent::StreamOpened {
                    group_id,
                    subgroup_id,
                }) => {
                    if let Some(start_group_id) = absolute_group_id {
                        let target_group = next_absolute_group.unwrap_or(start_group_id);
                        let should_send = match self.group_order {
                            GroupOrder::Descending => group_id >= target_group,
                            _ => group_id == target_group,
                        };
                        if !should_send {
                            continue;
                        }
                        let offset = if group_id == start_group_id {
                            start_offset_remaining.take().unwrap_or(0)
                        } else {
                            0
                        };
                        next_absolute_group = self
                            .schedule_stream_groups_from(
                                group_id,
                                subgroup_id,
                                offset,
                                &mut scheduled,
                            )
                            .await
                            .or(Some(group_id + 1));
                        continue;
                    }

                    if matches!(self.filter_type, FilterType::NextGroupStart)
                        && next_group_start_floor.is_some_and(|floor| group_id <= floor)
                    {
                        continue;
                    }

                    let _ = self
                        .schedule_stream_task(group_id, subgroup_id, 0, &mut scheduled)
                        .await;
                }
                Ok(TrackEvent::DatagramOpened { group_id }) => {
                    if let Some(start_group_id) = absolute_group_id {
                        let target_group = next_absolute_group.unwrap_or(start_group_id);
                        let should_send = match self.group_order {
                            GroupOrder::Descending => group_id >= target_group,
                            _ => group_id == target_group,
                        };
                        if !should_send {
                            continue;
                        }
                        let offset = if group_id == start_group_id {
                            start_offset_remaining.take().unwrap_or(0)
                        } else {
                            0
                        };
                        next_absolute_group = self
                            .schedule_datagram_groups_from(group_id, offset, &mut scheduled)
                            .await
                            .or(Some(group_id + 1));
                        continue;
                    }

                    if matches!(self.filter_type, FilterType::NextGroupStart)
                        && next_group_start_floor.is_some_and(|floor| group_id <= floor)
                    {
                        continue;
                    }

                    let _ = self
                        .schedule_datagram_task(group_id, 0, &mut scheduled)
                        .await;
                }
                Ok(TrackEvent::EndOfGroup) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress scheduler receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    async fn schedule_location(
        &self,
        location: CacheLocation,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) {
        match location {
            CacheLocation::Stream {
                group_id,
                subgroup_id,
                index,
            } => {
                let _ = self
                    .schedule_stream_task(group_id, subgroup_id, index, scheduled)
                    .await;
            }
            CacheLocation::Datagram { group_id, index } => {
                let _ = self
                    .schedule_datagram_task(group_id, index, scheduled)
                    .await;
            }
        }
    }

    async fn schedule_stream_task(
        &self,
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        start_offset: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<()> {
        let task = GroupSendTask::Stream {
            group_id,
            subgroup_id,
            start_offset,
        };
        if !scheduled.insert(task.key()) {
            return Some(());
        }
        self.sender.send(task).await.ok()?;
        Some(())
    }

    async fn schedule_datagram_task(
        &self,
        group_id: u64,
        start_offset: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<()> {
        let task = GroupSendTask::Datagram {
            group_id,
            start_offset,
        };
        if !scheduled.insert(task.key()) {
            return Some(());
        }
        self.sender.send(task).await.ok()?;
        Some(())
    }

    async fn schedule_stream_groups_from(
        &self,
        from_group_id: u64,
        first_subgroup_id: StreamSubgroupId,
        first_offset: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<u64> {
        self.schedule_stream_task(from_group_id, first_subgroup_id, first_offset, scheduled)
            .await?;

        if matches!(self.group_order, GroupOrder::Descending) {
            return Some(from_group_id + 1);
        }

        let mut next = from_group_id + 1;
        while self.cache.has_stream_group(next).await {
            for subgroup_id in self.cache.stream_subgroups(next).await {
                let _ = self
                    .schedule_stream_task(next, subgroup_id, 0, scheduled)
                    .await;
            }
            next += 1;
        }
        Some(next)
    }

    async fn schedule_datagram_groups_from(
        &self,
        from_group_id: u64,
        first_offset: u64,
        scheduled: &mut HashSet<GroupSendTaskKey>,
    ) -> Option<u64> {
        self.schedule_datagram_task(from_group_id, first_offset, scheduled)
            .await?;

        if matches!(self.group_order, GroupOrder::Descending) {
            return Some(from_group_id + 1);
        }

        let mut next = from_group_id + 1;
        while self.cache.has_datagram_group(next).await {
            let _ = self.schedule_datagram_task(next, 0, scheduled).await;
            next += 1;
        }
        Some(next)
    }

    fn resolve_absolute_start(filter_type: &FilterType) -> Option<(u64, u64)> {
        match filter_type {
            FilterType::AbsoluteStart { location } => Some((location.group_id, location.object_id)),
            FilterType::AbsoluteRange { location, .. } => {
                Some((location.group_id, location.object_id))
            }
            _ => None,
        }
    }
}

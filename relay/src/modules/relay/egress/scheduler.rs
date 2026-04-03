use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::modules::{
    enums::{FilterType, GroupOrder},
    relay::{cache::track_cache::TrackCache, notifications::latest_info::LatestInfo},
};

/// GroupSender へ送るグループ送信指示
pub(crate) struct GroupSendTask {
    pub(crate) group_id: u64,
    pub(crate) start_offset: u64,
    pub(crate) is_stream: bool,
}

/// LatestInfo を監視して「どのグループをいつ送るか」を決定し、
/// GroupSender へ GroupSendTask を送信する。
pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    filter_type: FilterType,
    group_order: GroupOrder,
    sender: mpsc::Sender<GroupSendTask>,
}

impl EgressScheduler {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
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
        let (start_group_id, start_offset) =
            Self::resolve_start(&self.cache, &self.filter_type).await;
        // eager schedule より前に subscribe してイベントを取りこぼさないようにする
        let mut receiver = self.latest_info_sender.subscribe();

        let is_absolute = matches!(
            self.filter_type,
            FilterType::AbsoluteStart { .. } | FilterType::AbsoluteRange { .. }
        );
        let mut next_absolute_group: Option<u64> = None;

        if is_absolute {
            // 指定 group_id が届くまで待機するだけ。送信は on_group_opened で行う。
            next_absolute_group = Some(start_group_id);
        }

        loop {
            match receiver.recv().await {
                Ok(LatestInfo::StreamOpened { group_id }) => {
                    next_absolute_group = self
                        .on_group_opened(
                            group_id,
                            true,
                            is_absolute,
                            next_absolute_group,
                            start_group_id,
                            start_offset,
                        )
                        .await;
                }
                Ok(LatestInfo::DatagramOpened { group_id }) => {
                    next_absolute_group = self
                        .on_group_opened(
                            group_id,
                            false,
                            is_absolute,
                            next_absolute_group,
                            start_group_id,
                            start_offset,
                        )
                        .await;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress scheduler receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// グループ到着時の統一ハンドラ（StreamOpened / DatagramOpened 共通）
    /// 戻り値 = 更新後の next_absolute_group
    async fn on_group_opened(
        &self,
        group_id: u64,
        is_stream: bool,
        is_absolute: bool,
        next_absolute_group: Option<u64>,
        start_group_id: u64,
        start_offset: u64,
    ) -> Option<u64> {
        if is_absolute {
            // Descending: group_id >= next ならジャンプを許容して送信
            // Ascending:  group_id == next のみ送信（ギャップを待つ）
            let should_send = match self.group_order {
                GroupOrder::Descending => group_id >= next_absolute_group.unwrap_or(0),
                _ => next_absolute_group == Some(group_id),
            };
            if !should_send {
                return next_absolute_group;
            }
            let offset = if group_id == start_group_id {
                start_offset
            } else {
                0
            };
            let next = self
                .schedule_groups_from(group_id, offset, is_stream)
                .await
                .unwrap_or(group_id + 1);
            Some(next)
        } else {
            let offset = if group_id == start_group_id {
                start_offset
            } else {
                0
            };
            let _ = self
                .sender
                .send(GroupSendTask {
                    group_id,
                    start_offset: offset,
                    is_stream,
                })
                .await;
            next_absolute_group
        }
    }

    /// from_group_id から連続するキャッシュ内グループをスケジュールする
    ///
    /// - Ascending: キャッシュ内の後続グループを順番に全て送信
    /// - Descending: from_group_id のみ送信（最新追従のためキャッシュスキャンなし）
    /// - from_group_id の送信に失敗した場合は None を返す
    /// - 戻り値 Some(n) = 次に待つべき group_id
    async fn schedule_groups_from(
        &self,
        from_group_id: u64,
        first_offset: u64,
        is_stream: bool,
    ) -> Option<u64> {
        self.sender
            .send(GroupSendTask {
                group_id: from_group_id,
                start_offset: first_offset,
                is_stream,
            })
            .await
            .ok()?;

        if matches!(self.group_order, GroupOrder::Descending) {
            return Some(from_group_id + 1);
        }

        let mut next = from_group_id + 1;
        while self.cache.has_group(next).await {
            let _ = self
                .sender
                .send(GroupSendTask {
                    group_id: next,
                    start_offset: 0,
                    is_stream,
                })
                .await;
            next += 1;
        }
        Some(next)
    }

    async fn resolve_start(cache: &Arc<TrackCache>, filter_type: &FilterType) -> (u64, u64) {
        match filter_type {
            FilterType::NextGroupStart => {
                let group_id = cache.latest_group_id().await.unwrap_or(0);
                (group_id, 0)
            }
            FilterType::LargestObject => {
                let loc = cache.latest_location().await;
                loc.map(|l| (l.group_id, l.index)).unwrap_or((0, 0))
            }
            FilterType::AbsoluteStart { location } => (location.group_id, location.object_id),
            FilterType::AbsoluteRange { location, .. } => (location.group_id, location.object_id),
        }
    }
}

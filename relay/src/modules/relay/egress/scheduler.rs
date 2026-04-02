use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::modules::{
    enums::FilterType,
    relay::{
        cache::track_cache::TrackCache,
        caches::{delivery_type_map::DeliveryTypeMap, latest_info::LatestInfo},
    },
    types::TrackKey,
};

/// GroupSender へ送るグループ送信指示
pub(crate) struct GroupSendRequest {
    pub(crate) group_id: u64,
    pub(crate) start_offset: u64,
    pub(crate) is_stream: bool,
}

/// LatestInfo を監視して「どのグループをいつ送るか」を決定し、
/// GroupSender へ GroupSendRequest を送信する。
pub(crate) struct EgressScheduler {
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    delivery_type_map: Arc<DeliveryTypeMap>,
    track_key: TrackKey,
    filter_type: FilterType,
    sender: mpsc::Sender<GroupSendRequest>,
}

impl EgressScheduler {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
        delivery_type_map: Arc<DeliveryTypeMap>,
        filter_type: FilterType,
        sender: mpsc::Sender<GroupSendRequest>,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            delivery_type_map,
            filter_type,
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
            let Some(is_stream) = self.delivery_type_map.is_stream(self.track_key) else {
                tracing::error!(
                    track_key = self.track_key,
                    "delivery type unknown for absolute egress"
                );
                return;
            };
            let Some(next) = self
                .schedule_groups_from(start_group_id, start_offset, is_stream)
                .await
            else {
                tracing::error!("failed to schedule absolute egress start group");
                return;
            };
            next_absolute_group = Some(next);
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
            if next_absolute_group != Some(group_id) {
                return next_absolute_group;
            }
            let next = self
                .schedule_groups_from(group_id, 0, is_stream)
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
                .send(GroupSendRequest {
                    group_id,
                    start_offset: offset,
                    is_stream,
                })
                .await;
            next_absolute_group
        }
    }

    /// from_group_id から連続するキャッシュ内グループ全てのスケジュールを送信する
    ///
    /// - from_group_id の送信に失敗した場合は None を返す
    /// - 後続グループの失敗はスキップ（ベストエフォート）
    /// - 戻り値 Some(n) = 次に待つべき group_id
    async fn schedule_groups_from(
        &self,
        from_group_id: u64,
        first_offset: u64,
        is_stream: bool,
    ) -> Option<u64> {
        self.sender
            .send(GroupSendRequest {
                group_id: from_group_id,
                start_offset: first_offset,
                is_stream,
            })
            .await
            .ok()?;

        let mut next = from_group_id + 1;
        while self.cache.has_group(next).await {
            let _ = self
                .sender
                .send(GroupSendRequest {
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
            FilterType::LatestGroup => {
                let group_id = cache.latest_group_id().await.unwrap_or(0);
                (group_id, 0)
            }
            FilterType::LatestObject => {
                let loc = cache.latest_location().await;
                loc.map(|l| (l.group_id, l.index)).unwrap_or((0, 0))
            }
            FilterType::AbsoluteStart { location } => (location.group_id, location.object_id),
            FilterType::AbsoluteRange { location, .. } => (location.group_id, location.object_id),
        }
    }
}

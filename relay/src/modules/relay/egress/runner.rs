use std::sync::Arc;

use tokio::{sync::broadcast, task::JoinSet};

use crate::modules::{
    core::{data_sender::DataSender, published_resource::PublishedResource, publisher::Publisher},
    enums::FilterType,
    relay::{
        cache::track_cache::TrackCache,
        caches::{delivery_type_map::DeliveryTypeMap, latest_info::LatestInfo},
    },
    types::TrackKey,
};

pub(crate) struct EgressRunner {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    delivery_type_map: Arc<DeliveryTypeMap>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
        delivery_type_map: Arc<DeliveryTypeMap>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            delivery_type_map,
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let filter_type = self.published_resource.filter_type();
        let subscriber_track_alias = self.published_resource.track_alias();
        let mut joinset = JoinSet::<Option<u64>>::new();

        let (start_group_id, start_offset) = Self::resolve_start(&self.cache, &filter_type).await;
        // eager spawn より前に subscribe してイベントを取りこぼさないようにする
        let mut latest_info_receiver = self.latest_info_sender.subscribe();

        let is_absolute = matches!(
            filter_type,
            FilterType::AbsoluteStart { .. } | FilterType::AbsoluteRange { .. }
        );
        // Absolute 用: 次にスポーンすべきグループ ID
        let mut next_absolute_group: Option<u64> = None;

        if is_absolute {
            let Some(is_stream) = self.delivery_type_map.is_stream(self.track_key) else {
                tracing::error!(
                    track_key = self.track_key,
                    "delivery type unknown for absolute egress"
                );
                return Ok(());
            };

            // start_group をスポーン
            match self.new_sender(is_stream, subscriber_track_alias).await {
                Some(sender) => {
                    joinset.spawn(Self::stream_send_task(
                        start_group_id,
                        start_offset,
                        self.cache.clone(),
                        sender,
                    ));
                }
                None => {
                    tracing::error!("failed to open sender for absolute egress start group");
                    return Ok(());
                }
            }

            // キャッシュに既に存在する後続グループを即座に並列スポーン
            let mut next = start_group_id + 1;
            while self.cache.has_group(next).await {
                if let Some(sender) = self.new_sender(is_stream, subscriber_track_alias).await {
                    joinset.spawn(Self::stream_send_task(next, 0, self.cache.clone(), sender));
                }
                next += 1;
            }
            next_absolute_group = Some(next);
        }

        loop {
            tokio::select! {
                result = latest_info_receiver.recv() => {
                    match result {
                        Ok(LatestInfo::StreamOpened { track_key, group_id }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            if is_absolute {
                                // Absolute: 次に期待するグループが到着したときのみスポーン
                                if next_absolute_group == Some(group_id) {
                                    if let Ok(sender) = self
                                        .publisher
                                        .new_stream(&self.published_resource, subscriber_track_alias)
                                        .await
                                    {
                                        joinset.spawn(Self::stream_send_task(
                                            group_id, 0, self.cache.clone(), sender,
                                        ));
                                    }
                                    // キャッシュ内の更なる後続グループも即座にスポーン
                                    let mut next = group_id + 1;
                                    while self.cache.has_group(next).await {
                                        if let Ok(sender) = self
                                            .publisher
                                            .new_stream(&self.published_resource, subscriber_track_alias)
                                            .await
                                        {
                                            joinset.spawn(Self::stream_send_task(
                                                next, 0, self.cache.clone(), sender,
                                            ));
                                        }
                                        next += 1;
                                    }
                                    next_absolute_group = Some(next);
                                }
                            } else {
                                // LatestGroup/LatestObject
                                let offset =
                                    if group_id == start_group_id { start_offset } else { 0 };
                                match self
                                    .publisher
                                    .new_stream(&self.published_resource, subscriber_track_alias)
                                    .await
                                {
                                    Ok(sender) => {
                                        joinset.spawn(Self::stream_send_task(
                                            group_id, offset, self.cache.clone(), sender,
                                        ));
                                    }
                                    Err(e) => {
                                        tracing::error!(?e, "failed to open stream for egress");
                                    }
                                }
                            }
                        }
                        Ok(LatestInfo::DatagramOpened { track_key, group_id }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            if is_absolute {
                                // Absolute: 次に期待するグループが到着したときのみスポーン
                                if next_absolute_group == Some(group_id) {
                                    let sender =
                                        self.publisher.new_datagram(&self.published_resource);
                                    joinset.spawn(Self::stream_send_task(
                                        group_id, 0, self.cache.clone(), sender,
                                    ));
                                    // キャッシュ内の更なる後続グループも即座にスポーン
                                    let mut next = group_id + 1;
                                    while self.cache.has_group(next).await {
                                        let sender =
                                            self.publisher.new_datagram(&self.published_resource);
                                        joinset.spawn(Self::stream_send_task(
                                            next, 0, self.cache.clone(), sender,
                                        ));
                                        next += 1;
                                    }
                                    next_absolute_group = Some(next);
                                }
                            } else {
                                // LatestGroup/LatestObject
                                let offset =
                                    if group_id == start_group_id { start_offset } else { 0 };
                                let sender =
                                    self.publisher.new_datagram(&self.published_resource);
                                joinset.spawn(Self::stream_send_task(
                                    group_id, offset, self.cache.clone(), sender,
                                ));
                            }
                        }
                        Ok(LatestInfo::LatestObject { .. } | LatestInfo::EndOfGroup { .. }) => {}
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(n, "latest_info_receiver lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                Some(result) = joinset.join_next() => {
                    if let Err(e) = result {
                        tracing::error!("egress send task panicked: {:?}", e);
                    }
                }
            }
        }

        Ok(())
    }

    /// stream / datagram に応じた DataSender を生成する
    async fn new_sender(
        &self,
        is_stream: bool,
        subscriber_track_alias: u64,
    ) -> Option<Box<dyn DataSender>> {
        if is_stream {
            match self
                .publisher
                .new_stream(&self.published_resource, subscriber_track_alias)
                .await
            {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::error!(?e, "failed to open stream sender");
                    None
                }
            }
        } else {
            Some(self.publisher.new_datagram(&self.published_resource))
        }
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

    /// グループ内の全オブジェクトを送信するタスク（stream・datagram 共通）
    ///
    /// get_object_or_wait でグループがクローズされるまで待機しながらオブジェクトを送信する。
    /// stream の場合は QUIC ストリームが、datagram の場合は個別データグラムが送信される。
    async fn stream_send_task(
        group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) -> Option<u64> {
        let mut next_index = start_offset;
        loop {
            match cache.get_object_or_wait(group_id, next_index).await {
                Some(object) => {
                    if sender.send_object((*object).clone()).await.is_err() {
                        return Some(group_id);
                    }
                    next_index += 1;
                }
                None => break,
            }
        }
        Some(group_id)
    }
}

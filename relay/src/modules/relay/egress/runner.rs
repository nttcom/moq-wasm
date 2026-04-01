use std::sync::Arc;

use tokio::{sync::broadcast, task::JoinSet};

use crate::modules::{
    core::{data_sender::DataSender, published_resource::PublishedResource, publisher::Publisher},
    enums::FilterType,
    relay::{
        cache::track_cache::TrackCache, caches::latest_info::LatestInfo, egress::msg::EgressMsg,
    },
    types::TrackKey,
};

pub(crate) struct EgressRunner {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let filter_type = self.published_resource.filter_type();
        let subscriber_track_alias = self.published_resource.track_alias();
        let (egress_tx, _) = broadcast::channel::<EgressMsg>(256);
        let mut joinset = JoinSet::<()>::new();
        let mut datagram_spawned = false;

        match &filter_type {
            FilterType::AbsoluteStart { location } | FilterType::AbsoluteRange { location, .. } => {
                // 指定位置からどの filter_type でもストリームを即座に開通してドレイン
                let start_group_id = location.group_id;
                let start_offset = location.object_id;
                match self
                    .publisher
                    .new_stream(&self.published_resource, subscriber_track_alias)
                    .await
                {
                    Ok(stream_sender) => {
                        let rx = egress_tx.subscribe();
                        let cache = self.cache.clone();
                        joinset.spawn(Self::stream_send_task(
                            start_group_id,
                            start_group_id,
                            start_offset,
                            cache,
                            stream_sender,
                            rx,
                        ));
                    }
                    Err(e) => {
                        tracing::error!(?e, "failed to open stream for absolute egress");
                        return Ok(());
                    }
                }
            }
            FilterType::LatestGroup | FilterType::LatestObject => {
                // LatestInfo を待ってからストリームを開通する（下のループで処理）
            }
        }

        // 起動時の start 位置 (LatestGroup/LatestObject 用)
        let (start_group_id, start_offset) = Self::resolve_start(&self.cache, &filter_type).await;

        let mut latest_info_receiver = self.latest_info_sender.subscribe();

        loop {
            tokio::select! {
                result = latest_info_receiver.recv() => {
                    match result {
                        Ok(LatestInfo::StreamOpened { track_key, group_id }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            match self
                                .publisher
                                .new_stream(&self.published_resource, subscriber_track_alias)
                                .await
                            {
                                Ok(stream_sender) => {
                                    let rx = egress_tx.subscribe();
                                    let cache = self.cache.clone();
                                    joinset.spawn(Self::stream_send_task(
                                        group_id,
                                        start_group_id,
                                        start_offset,
                                        cache,
                                        stream_sender,
                                        rx,
                                    ));
                                }
                                Err(e) => {
                                    tracing::error!(?e, "failed to open stream for egress");
                                }
                            }
                        }
                        Ok(LatestInfo::DatagramOpened { track_key, .. }) => {
                            if track_key != self.track_key || datagram_spawned {
                                continue;
                            }
                            datagram_spawned = true;
                            let datagram_sender =
                                self.publisher.new_datagram(&self.published_resource);
                            let rx = egress_tx.subscribe();
                            let cache = self.cache.clone();
                            joinset.spawn(Self::datagram_send_task(datagram_sender, rx, cache));
                        }
                        Ok(LatestInfo::LatestObject { track_key, group_id, offset }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            let _ = egress_tx.send(EgressMsg::Object { group_id, offset });
                        }
                        Ok(LatestInfo::EndOfGroup { track_key, group_id }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            let _ = egress_tx.send(EgressMsg::Close { group_id });
                        }
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

    /// stream 送信スレッド
    ///
    /// 起動直後はキャッシュ内の既存データを先に送信し、
    /// 以降は broadcast::Receiver<EgressMsg> をリッスンする。
    /// ドレイン済み offset 未満の Object はスキップして重複を防ぐ。
    async fn stream_send_task(
        group_id: u64,
        start_group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
        mut receiver: broadcast::Receiver<EgressMsg>,
    ) {
        let cache_start = if group_id == start_group_id {
            start_offset
        } else {
            0
        };
        let mut next_index = cache_start;

        // キャッシュドレイン: グループがクローズされるまで全オブジェクトを待機しながら送信
        loop {
            match cache.get_object_or_wait(group_id, next_index).await {
                Some(object) => {
                    if sender.send_object((*object).clone()).await.is_err() {
                        return;
                    }
                    next_index += 1;
                }
                None => break, // グループクローズ確定
            }
        }

        // broadcast をリッスンして以降のオブジェクトを受信
        loop {
            match receiver.recv().await {
                Ok(EgressMsg::Object {
                    group_id: gid,
                    offset,
                }) if gid == group_id => {
                    // ドレイン済み offset はスキップ（重複防止）
                    if offset < next_index {
                        continue;
                    }
                    match cache.get_object(group_id, offset).await {
                        Some(object) => {
                            if sender.send_object((*object).clone()).await.is_err() {
                                return;
                            }
                            next_index = offset + 1;
                        }
                        None => {
                            tracing::warn!(group_id, offset, "cache miss in stream_send_task");
                        }
                    }
                }
                Ok(EgressMsg::Object { .. }) => {}
                Ok(EgressMsg::Close { group_id: gid }) if gid == group_id => return,
                Ok(EgressMsg::Close { .. }) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(group_id, n, "egress stream receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    /// datagram 送信スレッド
    ///
    /// DatagramOpened は初回のみ。以降は Object をキャッシュから引いて送信し続ける。
    async fn datagram_send_task(
        mut sender: Box<dyn DataSender>,
        mut receiver: broadcast::Receiver<EgressMsg>,
        cache: Arc<TrackCache>,
    ) {
        loop {
            match receiver.recv().await {
                Ok(EgressMsg::Object { group_id, offset }) => {
                    if let Some(object) = cache.get_object(group_id, offset).await {
                        if sender.send_object((*object).clone()).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(EgressMsg::Close { .. }) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress datagram receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

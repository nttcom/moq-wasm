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
    latest_info_receiver: broadcast::Receiver<LatestInfo>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_receiver: broadcast::Receiver<LatestInfo>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_receiver,
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        // 起動時: FilterType から start 位置を計算
        let filter_type = self.published_resource.filter_type();
        let (start_group_id, start_offset) = Self::resolve_start(&self.cache, &filter_type).await;
        let subscriber_track_alias = self.published_resource.track_alias();

        let (egress_tx, _) = broadcast::channel::<EgressMsg>(256);
        let mut joinset = JoinSet::<()>::new();
        let mut datagram_spawned = false;

        loop {
            tokio::select! {
                result = self.latest_info_receiver.recv() => {
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
                            joinset.spawn(Self::datagram_send_task(datagram_sender, rx));
                        }
                        Ok(LatestInfo::LatestObject {
                            track_key,
                            group_id,
                            offset,
                        }) => {
                            if track_key != self.track_key {
                                continue;
                            }
                            if let Some(data) = self.cache.get_object(group_id, offset).await {
                                let _ = egress_tx
                                    .send(EgressMsg::Object { group_id, data });
                            }
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
    /// group_id が一致するオブジェクトのみ処理し、Close を受けたら終了する。
    async fn stream_send_task(
        group_id: u64,
        start_group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
        mut receiver: broadcast::Receiver<EgressMsg>,
    ) {
        // 過去データ送信
        let cache_start = if group_id == start_group_id {
            start_offset
        } else {
            0
        };
        let mut index = cache_start;
        loop {
            match cache.get_object(group_id, index).await {
                Some(object) => {
                    if sender.send_object((*object).clone()).await.is_err() {
                        return;
                    }
                    index += 1;
                }
                None => break,
            }
        }

        // broadcast をリッスンして以降のオブジェクトを受信
        loop {
            match receiver.recv().await {
                Ok(EgressMsg::Object {
                    group_id: gid,
                    data,
                    ..
                }) if gid == group_id => {
                    if sender.send_object((*data).clone()).await.is_err() {
                        return;
                    }
                }
                Ok(EgressMsg::Object { .. }) => {
                    // group_id が一致しないのでスキップ
                }
                Ok(EgressMsg::Close { group_id: gid }) if gid == group_id => {
                    return;
                }
                Ok(EgressMsg::Close { .. }) => {
                    // 自分の group_id でないのでスキップ
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(group_id, n, "egress stream receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    /// datagram 送信スレッド
    ///
    /// Object を 1 回受け取ったら即終了する。
    async fn datagram_send_task(
        mut sender: Box<dyn DataSender>,
        mut receiver: broadcast::Receiver<EgressMsg>,
    ) {
        loop {
            match receiver.recv().await {
                Ok(EgressMsg::Object { data, .. }) => {
                    let _ = sender.send_object((*data).clone()).await;
                    return;
                }
                Ok(EgressMsg::Close { .. }) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "egress datagram receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

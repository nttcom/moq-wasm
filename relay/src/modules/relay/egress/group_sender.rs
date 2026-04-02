use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinSet};

use crate::modules::{
    core::{data_sender::DataSender, published_resource::PublishedResource, publisher::Publisher},
    relay::cache::track_cache::TrackCache,
};

use super::scheduler::GroupSendRequest;

/// GroupSendRequest を受け取り、実際の送信タスクを起動・管理する。
pub(crate) struct GroupSender {
    cache: Arc<TrackCache>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
    receiver: mpsc::Receiver<GroupSendRequest>,
}

impl GroupSender {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
        receiver: mpsc::Receiver<GroupSendRequest>,
    ) -> Self {
        Self {
            cache,
            publisher,
            published_resource,
            receiver,
        }
    }

    pub(crate) async fn run(mut self) {
        let subscriber_track_alias = self.published_resource.track_alias();
        let mut joinset = JoinSet::<Option<u64>>::new();

        loop {
            tokio::select! {
                Some(req) = self.receiver.recv() => {
                    if let Some(sender) = self.new_sender(req.is_stream, subscriber_track_alias).await {
                        joinset.spawn(Self::send_task(
                            req.group_id,
                            req.start_offset,
                            self.cache.clone(),
                            sender,
                        ));
                    }
                }
                Some(result) = joinset.join_next() => {
                    if let Err(e) = result {
                        tracing::error!("egress send task panicked: {:?}", e);
                    }
                }
                else => break,
            }
        }
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

    /// グループ内の全オブジェクトを送信するタスク（stream・datagram 共通）
    async fn send_task(
        group_id: u64,
        start_offset: u64,
        cache: Arc<TrackCache>,
        mut sender: Box<dyn DataSender>,
    ) -> Option<u64> {
        let mut next_index = start_offset;
        while let Some(object) = cache.get_object_or_wait(group_id, next_index).await {
            if sender.send_object((*object).clone()).await.is_err() {
                return Some(group_id);
            }
            next_index += 1;
        }
        Some(group_id)
    }
}

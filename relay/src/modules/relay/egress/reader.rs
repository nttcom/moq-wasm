use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use tokio::sync::broadcast;

use crate::modules::{
    core::{data_object::DataObject, data_sender::DataSender},
    enums::{FilterType, GroupOrder},
    relay::{
        cache::{latest_event::LatestCacheEvent, track_cache::TrackCache},
        egress::{reader_cursor::ReaderCursor, stream_allocator::StreamAllocator},
        types::RelayTransport,
    },
    types::TrackKey,
};

pub(crate) enum ReaderStep {
    EnsureStream {
        group_id: u64,
    },
    EnsureDatagram,
    SendObject {
        group_id: u64,
        index: u64,
        object: Arc<DataObject>,
    },
    Finished,
}

pub(crate) struct SubscriberAcceptReader {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    latest_receiver: broadcast::Receiver<LatestCacheEvent>,
    cursor: ReaderCursor,
    transport: RelayTransport,
    opened_stream_groups: BTreeSet<u64>,
    stream_senders: BTreeMap<u64, Box<dyn DataSender>>,
    datagram_sender: Option<Box<dyn DataSender>>,
}

impl SubscriberAcceptReader {
    pub(crate) async fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        filter_type: &FilterType,
        group_order: GroupOrder,
        transport: RelayTransport,
    ) -> Self {
        let (start_group_id, start_index, end_group) =
            Self::resolve_start(&cache, filter_type, &transport).await;
        Self {
            track_key,
            latest_receiver: cache.subscribe_latest(),
            cache,
            cursor: ReaderCursor::new(start_group_id, start_index, end_group, group_order),
            transport,
            opened_stream_groups: BTreeSet::new(),
            stream_senders: BTreeMap::new(),
            datagram_sender: None,
        }
    }

    async fn resolve_start(
        cache: &Arc<TrackCache>,
        filter_type: &FilterType,
        transport: &RelayTransport,
    ) -> (u64, u64, Option<u64>) {
        match filter_type {
            FilterType::LatestGroup => (cache.latest_group_id().await.unwrap_or(0), 0, None),
            FilterType::LatestObject => {
                let latest_group_id = cache
                    .latest_location()
                    .await
                    .map(|location| location.group_id)
                    .unwrap_or(0);
                (latest_group_id, 0, None)
            }
            FilterType::AbsoluteStart { location } => {
                let mut start_index = cache.resolve_index(location).await.unwrap_or(0);
                if matches!(transport, RelayTransport::Stream) {
                    start_index = 0;
                }
                (location.group_id, start_index, None)
            }
            FilterType::AbsoluteRange {
                location,
                end_group,
            } => {
                let mut start_index = cache.resolve_index(location).await.unwrap_or(0);
                if matches!(transport, RelayTransport::Stream) {
                    start_index = 0;
                }
                (location.group_id, start_index, Some(*end_group))
            }
        }
    }

    async fn next_cached_object(&mut self) -> Option<(u64, u64, Arc<DataObject>)> {
        loop {
            if self.cursor.has_passed_end_range() {
                return None;
            }

            let location = self.cursor.location();
            if let Some(object) = self
                .cache
                .get_object(location.group_id, location.index)
                .await
            {
                return Some((location.group_id, location.index, object));
            }

            if self.cache.is_group_closed(location.group_id).await
                && let Some(next_group_id) = self
                    .cache
                    .next_group_id(location.group_id, self.cursor.group_order())
                    .await
            {
                self.cursor.jump_to_group(next_group_id);
                continue;
            }

            match self.latest_receiver.recv().await {
                Ok(LatestCacheEvent::ObjectAppended {
                    track_key,
                    group_id,
                    index,
                }) => {
                    tracing::trace!(
                        track_key,
                        group_id,
                        index,
                        reader_track_key = self.track_key,
                        "latest object appended"
                    );
                }
                Ok(LatestCacheEvent::GroupClosed {
                    track_key,
                    group_id,
                }) => {
                    tracing::trace!(
                        track_key,
                        group_id,
                        reader_track_key = self.track_key,
                        "group closed"
                    );
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        reader_track_key = self.track_key,
                        skipped,
                        "latest cache receiver lagged"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }

    pub(crate) async fn next_step(&mut self) -> ReaderStep {
        let Some((group_id, index, object)) = self.next_cached_object().await else {
            return ReaderStep::Finished;
        };

        if matches!(self.transport, RelayTransport::Stream)
            && matches!(object.as_ref(), DataObject::SubgroupHeader(_))
            && (!self.opened_stream_groups.contains(&group_id)
                || self.cursor.is_header_sent(group_id))
        {
            return ReaderStep::EnsureStream { group_id };
        }

        if matches!(self.transport, RelayTransport::Datagram) && self.datagram_sender.is_none() {
            return ReaderStep::EnsureDatagram;
        }

        if matches!(self.transport, RelayTransport::Stream)
            && !self.cursor.is_header_sent(group_id)
            && !matches!(object.as_ref(), DataObject::SubgroupHeader(_))
            && let Some(header) = self.cache.group_header(group_id).await
        {
            self.cursor.mark_header_sent(group_id);
            return ReaderStep::SendObject {
                group_id,
                index: 0,
                object: header,
            };
        }

        if matches!(object.as_ref(), DataObject::SubgroupHeader(_)) {
            self.cursor.mark_header_sent(group_id);
        }

        self.cursor.advance_object();
        ReaderStep::SendObject {
            group_id,
            index,
            object,
        }
    }

    pub(crate) fn register_stream_sender(&mut self, group_id: u64, sender: Box<dyn DataSender>) {
        self.stream_senders.insert(group_id, sender);
        self.opened_stream_groups.insert(group_id);
        self.cursor.reset_header_sent(group_id);
    }

    pub(crate) fn register_datagram_sender(&mut self, sender: Box<dyn DataSender>) {
        self.datagram_sender = Some(sender);
    }

    pub(crate) async fn run_with_allocator(
        &mut self,
        allocator: &StreamAllocator<'_>,
        subscriber_track_alias: u64,
    ) -> anyhow::Result<()> {
        loop {
            match self.next_step().await {
                ReaderStep::EnsureStream { group_id } => {
                    let sender = allocator.create_stream(subscriber_track_alias).await?;
                    self.register_stream_sender(group_id, sender);
                }
                ReaderStep::EnsureDatagram => {
                    let sender = allocator.create_datagram();
                    self.register_datagram_sender(sender);
                }
                ReaderStep::SendObject {
                    group_id,
                    index,
                    object,
                } => {
                    tracing::trace!(
                        track_key = self.track_key,
                        group_id,
                        index,
                        "sending object"
                    );
                    match self.transport {
                        RelayTransport::Stream => {
                            let sender = self
                                .stream_senders
                                .get_mut(&group_id)
                                .ok_or_else(|| anyhow::anyhow!("stream sender not initialized"))?;
                            sender.send_object((*object).clone()).await?;
                        }
                        RelayTransport::Datagram => {
                            let sender = self.datagram_sender.as_mut().ok_or_else(|| {
                                anyhow::anyhow!("datagram sender not initialized")
                            })?;
                            sender.send_object((*object).clone()).await?;
                        }
                    }
                }
                ReaderStep::Finished => return Ok(()),
            }
        }
    }
}

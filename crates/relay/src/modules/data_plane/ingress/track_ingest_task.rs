use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{cached_object::CacheStamp, store::TrackCacheStore, track_cache::TrackCache},
        ingress::track_reader::read_track,
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{data_receiver::receiver::DataReceiver, session_event::SessionEvent},
};

pub(crate) struct IngestStart {
    pub(crate) track_key: TrackKey,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) receiver: Box<dyn DataReceiver>,
    pub(crate) track_span: Span,
    pub(crate) stamp: CacheStamp,
}

pub(crate) enum IngestCommand {
    Start(IngestStart),
    Stop {
        track_key: TrackKey,
        publisher_session_id: SessionId,
    },
}

#[derive(Clone)]
pub(crate) struct TrackIngest {
    pub(crate) track_key: TrackKey,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) stop_receiver: watch::Receiver<bool>,
    pub(crate) stamp: CacheStamp,
}

type IngestKey = (TrackKey, SessionId);

pub(crate) struct TrackIngestTask {
    join_handle: JoinHandle<()>,
}

impl TrackIngestTask {
    pub(crate) fn run(
        mut receiver: mpsc::Receiver<IngestCommand>,
        cache_store: Arc<TrackCacheStore>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut ingests = JoinSet::new();
            let mut stop_senders = HashMap::<IngestKey, (u64, watch::Sender<bool>)>::new();
            let mut next_ingest_id = 0u64;
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => match command {
                        IngestCommand::Start(start) => {
                            let ingest_key = (start.track_key.clone(), start.publisher_session_id);
                            if stop_senders.contains_key(&ingest_key) {
                                tracing::warn!(track_key = %start.track_key, publisher_session_id = start.publisher_session_id, "ignoring a second ingest of the same publisher for an active track");
                                continue;
                            }
                            next_ingest_id += 1;
                            let ingest_id = next_ingest_id;
                            let (stop_sender, stop_receiver) = watch::channel(false);
                            stop_senders.insert(ingest_key.clone(), (ingest_id, stop_sender));
                            let ingest = TrackIngest {
                                cache: cache_store.get_or_create(&start.track_key),
                                track_key: start.track_key,
                                publisher_session_id: start.publisher_session_id,
                                session_event_sender: session_event_sender.clone(),
                                stop_receiver,
                                stamp: start.stamp,
                            };
                            ingests.spawn(async move {
                                Self::ingest(ingest, start.receiver, start.track_span).await;
                                (ingest_key, ingest_id)
                            });
                        }
                        IngestCommand::Stop { track_key, publisher_session_id } => {
                            if let Some((_, stop_sender)) = stop_senders.remove(&(track_key.clone(), publisher_session_id)) {
                                let _ = stop_sender.send(true);
                                tracing::info!(%track_key, publisher_session_id, "ingress track stop requested");
                            }
                        }
                    },
                    Some(result) = ingests.join_next() => match result {
                        Ok((ingest_key, ingest_id)) => {
                            if stop_senders.get(&ingest_key).is_some_and(|(id, _)| *id == ingest_id) {
                                stop_senders.remove(&ingest_key);
                            }
                            let (track_key, _) = ingest_key;
                            tracing::debug!(%track_key, "ingress track ended");
                        }
                        Err(e) => {
                            tracing::error!("ingress track task panicked: {:?}", e);
                        }
                    },
                    else => break,
                }
            }
        });
        Self { join_handle }
    }

    async fn ingest(ingest: TrackIngest, receiver: Box<dyn DataReceiver>, track_span: Span) {
        let cache = ingest.cache.clone();
        cache.begin_live_ingest();
        let span = tracing::debug_span!(
            parent: &track_span,
            "relay.dataplane.ingress.data_receiver",
            track_key = %ingest.track_key,
        );
        read_track(ingest, receiver, track_span)
            .instrument(span)
            .await;
        cache.end_live_ingest();
    }
}

impl Drop for TrackIngestTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::{
        UpstreamTrack, fixtures::location, wait_largest_location,
    };

    const FIRST_PUBLISHER: SessionId = 1;
    const SECOND_PUBLISHER: SessionId = 2;

    struct IngestContext {
        _task: TrackIngestTask,
        command_sender: mpsc::Sender<IngestCommand>,
        cache_store: Arc<TrackCacheStore>,
        _session_event_receiver: mpsc::UnboundedReceiver<SessionEvent>,
    }

    impl IngestContext {
        fn new() -> Self {
            let (command_sender, command_receiver) = mpsc::channel(8);
            let cache_store = Arc::new(TrackCacheStore::new());
            let (session_event_sender, session_event_receiver) = mpsc::unbounded_channel();
            Self {
                _task: TrackIngestTask::run(
                    command_receiver,
                    cache_store.clone(),
                    session_event_sender,
                ),
                command_sender,
                cache_store,
                _session_event_receiver: session_event_receiver,
            }
        }

        async fn start(&self, publisher_session_id: SessionId) -> UpstreamTrack {
            let (upstream, receiver) = UpstreamTrack::channel();
            self.command_sender
                .send(IngestCommand::Start(IngestStart {
                    track_key: track_key(),
                    publisher_session_id,
                    receiver,
                    track_span: Span::none(),
                    stamp: CacheStamp::default(),
                }))
                .await
                .unwrap();
            upstream
        }

        async fn stop(&self, publisher_session_id: SessionId) {
            self.command_sender
                .send(IngestCommand::Stop {
                    track_key: track_key(),
                    publisher_session_id,
                })
                .await
                .unwrap();
        }

        async fn wait_largest_location(&self, expected: moqt::object::Location) {
            wait_largest_location(&self.cache_store.get_or_create(&track_key()), expected).await;
        }
    }

    fn track_key() -> TrackKey {
        TrackKey::new("ns", "track")
    }

    #[tokio::test]
    async fn a_second_publisher_of_an_active_track_is_ingested() {
        // Arrange
        let context = IngestContext::new();
        let _first = context.start(FIRST_PUBLISHER).await;
        let second = context.start(SECOND_PUBLISHER).await;
        // Act
        let stream = second.open_stream();
        stream.header(0);
        stream.object(0);
        // Assert
        context.wait_largest_location(location(0, 0)).await;
    }

    #[tokio::test]
    async fn stopping_one_publisher_keeps_ingesting_the_other() {
        // Arrange
        let context = IngestContext::new();
        let first = context.start(FIRST_PUBLISHER).await;
        let second = context.start(SECOND_PUBLISHER).await;
        let first_stream = first.open_stream();
        first_stream.header(0);
        first_stream.object(0);
        context.wait_largest_location(location(0, 0)).await;
        // Act
        context.stop(FIRST_PUBLISHER).await;
        let second_stream = second.open_stream();
        second_stream.header(1);
        second_stream.object(0);
        // Assert
        context.wait_largest_location(location(1, 0)).await;
    }
}

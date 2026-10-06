use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{store::TrackCacheStore, track_cache::TrackCache},
        ingress::{datagram_reader::read_datagrams, stream_reader::accept_streams},
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{
        data_receiver::{
            datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiverFactory,
        },
        session_event::SessionEvent,
    },
};

pub(crate) enum IngestSource {
    Stream {
        factory: Box<dyn StreamReceiverFactory>,
        track_span: Span,
    },
    Datagram(Box<dyn DatagramReceiver>),
}

impl IngestSource {
    pub(crate) fn kind(&self) -> IngestKind {
        match self {
            Self::Stream { .. } => IngestKind::Stream,
            Self::Datagram(_) => IngestKind::Datagram,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IngestKind {
    Stream,
    Datagram,
}

impl IngestKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Datagram => "datagram",
        }
    }
}

pub(crate) struct IngestStart {
    pub(crate) track_key: TrackKey,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) source: IngestSource,
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
}

type IngestKey = (TrackKey, SessionId, IngestKind);

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
                            let ingest_key = (start.track_key.clone(), start.publisher_session_id, start.source.kind());
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
                            };
                            ingests.spawn(async move {
                                Self::ingest(ingest, start.source).await;
                                (ingest_key, ingest_id)
                            });
                        }
                        IngestCommand::Stop { track_key, publisher_session_id } => {
                            for kind in [IngestKind::Stream, IngestKind::Datagram] {
                                let ingest_key = (track_key.clone(), publisher_session_id, kind);
                                if let Some((_, stop_sender)) = stop_senders.remove(&ingest_key) {
                                    let _ = stop_sender.send(true);
                                    tracing::info!(%track_key, publisher_session_id, "{} ingress track stop requested", kind.label());
                                }
                            }
                        }
                    },
                    Some(result) = ingests.join_next() => match result {
                        Ok((ingest_key, ingest_id)) => {
                            if stop_senders.get(&ingest_key).is_some_and(|(id, _)| *id == ingest_id) {
                                stop_senders.remove(&ingest_key);
                            }
                            let (track_key, _, kind) = ingest_key;
                            tracing::debug!(%track_key, "{} ingress track ended", kind.label());
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

    async fn ingest(ingest: TrackIngest, source: IngestSource) {
        let cache = ingest.cache.clone();
        cache.begin_live_ingest();
        match source {
            IngestSource::Stream {
                factory,
                track_span,
            } => {
                let span = tracing::debug_span!(
                    parent: &track_span,
                    "relay.dataplane.ingress.stream_factory",
                    track_key = %ingest.track_key,
                );
                accept_streams(ingest, factory, track_span)
                    .instrument(span)
                    .await;
            }
            IngestSource::Datagram(receiver) => read_datagrams(ingest, receiver).await,
        }
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
    use crate::modules::{
        session::data_receiver::stream_receiver::StreamReceiver,
        test_support::relay_harness::{
            UpstreamSubgroupStream, fixtures::location, wait_largest_location,
        },
    };

    const FIRST_PUBLISHER: SessionId = 1;
    const SECOND_PUBLISHER: SessionId = 2;

    struct ChannelStreamReceiverFactory {
        stream_receiver: mpsc::UnboundedReceiver<Box<dyn StreamReceiver>>,
    }

    #[async_trait::async_trait]
    impl StreamReceiverFactory for ChannelStreamReceiverFactory {
        async fn next(&mut self) -> anyhow::Result<Box<dyn StreamReceiver>> {
            self.stream_receiver
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("upstream subscription ended"))
        }
    }

    struct UpstreamPublisher {
        stream_sender: mpsc::UnboundedSender<Box<dyn StreamReceiver>>,
    }

    impl UpstreamPublisher {
        fn open_stream(&self) -> UpstreamSubgroupStream {
            UpstreamSubgroupStream::open(|receiver| {
                self.stream_sender
                    .send(receiver)
                    .expect("ingest should accept streams");
                tokio::spawn(async {})
            })
        }
    }

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

        async fn start(&self, publisher_session_id: SessionId) -> UpstreamPublisher {
            let (stream_sender, stream_receiver) = mpsc::unbounded_channel();
            self.command_sender
                .send(IngestCommand::Start(IngestStart {
                    track_key: track_key(),
                    publisher_session_id,
                    source: IngestSource::Stream {
                        factory: Box::new(ChannelStreamReceiverFactory { stream_receiver }),
                        track_span: Span::none(),
                    },
                }))
                .await
                .unwrap();
            UpstreamPublisher { stream_sender }
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

        async fn wait_largest_location(&self, expected: moqt::Location) {
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

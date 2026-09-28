use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tracing::{Instrument, Span};

use crate::modules::{
    core::data_receiver::{
        datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiverFactory,
    },
    relay::{
        cache::{store::TrackCacheStore, track_cache::TrackCache},
        ingress::{datagram_reader::read_datagrams, stream_reader::accept_streams},
    },
    session_event::SessionEvent,
    types::{SessionId, TrackKey},
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

type IngestKey = (TrackKey, IngestKind);

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
            let mut stop_senders = HashMap::<IngestKey, (watch::Sender<bool>, SessionId)>::new();
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => match command {
                        IngestCommand::Start(start) => {
                            let ingest_key = (start.track_key.clone(), start.source.kind());
                            // draft-14 §8.2 Multiple Publishers: the first publisher wins and later
                            // ones are ignored, instead of the per-object dedup the SHOULD asks for.
                            if stop_senders.contains_key(&ingest_key) {
                                tracing::warn!(track_key = %start.track_key, "ignoring additional publisher for active track");
                                continue;
                            }
                            let (stop_sender, stop_receiver) = watch::channel(false);
                            stop_senders.insert(ingest_key.clone(), (stop_sender, start.publisher_session_id));
                            let ingest = TrackIngest {
                                cache: cache_store.get_or_create(&start.track_key),
                                track_key: start.track_key,
                                publisher_session_id: start.publisher_session_id,
                                session_event_sender: session_event_sender.clone(),
                                stop_receiver,
                            };
                            ingests.spawn(async move {
                                Self::ingest(ingest, start.source).await;
                                ingest_key
                            });
                        }
                        IngestCommand::Stop { track_key, publisher_session_id } => {
                            for kind in [IngestKind::Stream, IngestKind::Datagram] {
                                let ingest_key = (track_key.clone(), kind);
                                // Only the owning publisher may stop the reader, so a different
                                // publisher of the same track leaving does not tear down the active one.
                                if stop_senders.get(&ingest_key).is_some_and(|(_, owner)| *owner == publisher_session_id)
                                    && let Some((stop_sender, _)) = stop_senders.remove(&ingest_key)
                                {
                                    let _ = stop_sender.send(true);
                                    tracing::info!(%track_key, "{} ingress track stop requested", kind.label());
                                }
                            }
                        }
                    },
                    Some(result) = ingests.join_next() => match result {
                        Ok(ingest_key) => {
                            stop_senders.remove(&ingest_key);
                            let (track_key, kind) = ingest_key;
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

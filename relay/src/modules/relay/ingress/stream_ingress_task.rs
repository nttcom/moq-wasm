use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::modules::{
    core::data_receiver::stream_receiver::StreamReceiverFactory,
    relay::{
        cache::store::TrackCacheStore,
        ingress::stream_reader::{StreamOpened, StreamReader},
        notifications::track_notifier::TrackNotifier,
    },
    types::TrackKey,
};

pub(crate) struct StreamReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) factory: Box<dyn StreamReceiverFactory>,
    pub(crate) track_span: Span,
}

pub(crate) enum StreamIngressCommand {
    Start(StreamReceiveStart),
    Stop { track_key: TrackKey },
}

pub(crate) struct StreamIngressTask {
    join_handle: JoinHandle<()>,
    _stream_reader: StreamReader,
}

impl StreamIngressTask {
    pub(crate) fn new(
        mut receiver: mpsc::Receiver<StreamIngressCommand>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<TrackNotifier>,
    ) -> Self {
        let (opened_tx, opened_rx) = mpsc::channel::<StreamOpened>(64);
        let stream_reader = StreamReader::run(opened_rx, cache_store, sender_map);

        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            let mut stop_senders = HashMap::<TrackKey, watch::Sender<bool>>::new();
            loop {
                tokio::select! {
                    Some(command) = receiver.recv() => {
                        match command {
                            StreamIngressCommand::Start(cmd) => {
                                if let Some(stop_sender) = stop_senders.remove(&cmd.track_key) {
                                    let _ = stop_sender.send(true);
                                }

                                let (stop_sender, stop_receiver) = watch::channel(false);
                                stop_senders.insert(cmd.track_key, stop_sender);

                                let span = tracing::debug_span!(
                                    parent: &cmd.track_span,
                                    "relay.dataplane.ingress.stream_factory",
                                    track_key = cmd.track_key,
                                );
                                let track_key = cmd.track_key;
                                let opened_tx = opened_tx.clone();
                                joinset.spawn(async move {
                                    Self::factory_loop(
                                        track_key,
                                        cmd.factory,
                                        opened_tx,
                                        cmd.track_span,
                                        stop_receiver,
                                    )
                                    .await;
                                    track_key
                                }.instrument(span));
                            }
                            StreamIngressCommand::Stop { track_key } => {
                                if let Some(stop_sender) = stop_senders.remove(&track_key) {
                                    let _ = stop_sender.send(true);
                                    tracing::info!(track_key, "stream ingress track stop requested");
                                }
                            }
                        }
                    }
                    Some(result) = joinset.join_next() => {
                        match result {
                            Ok(track_key) => {
                                stop_senders.remove(&track_key);
                                tracing::debug!(track_key, "stream ingress track ended");
                            }
                            Err(e) => {
                                tracing::error!("stream accept task panicked: {:?}", e);
                            }
                        }
                    }
                    else => break,
                }
            }
        });
        Self {
            join_handle,
            _stream_reader: stream_reader,
        }
    }

    async fn factory_loop(
        track_key: TrackKey,
        mut factory: Box<dyn StreamReceiverFactory>,
        stream_tx: mpsc::Sender<StreamOpened>,
        track_span: Span,
        mut stop_receiver: watch::Receiver<bool>,
    ) {
        loop {
            let receiver = tokio::select! {
                _ = stop_receiver.changed() => {
                    tracing::info!(track_key, "stream ingress factory stopped");
                    return;
                }
                receiver = factory.next() => match receiver {
                    Ok(receiver) => receiver,
                    Err(_) => return,
                }
            };
            if stream_tx
                .send(StreamOpened {
                    track_key,
                    receiver,
                    parent_span: track_span.clone(),
                    stop_receiver: stop_receiver.clone(),
                })
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

impl Drop for StreamIngressTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

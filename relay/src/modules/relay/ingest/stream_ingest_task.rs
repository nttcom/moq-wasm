use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::data_receiver::stream_receiver::StreamReceiverFactory,
    relay::{
        cache::store::TrackCacheStore,
        ingest::stream_reader::{StreamOpened, StreamReader},
        notifications::sender_map::SenderMap,
    },
    types::TrackKey,
};

pub(crate) struct StreamReceiveStart {
    pub(crate) track_key: TrackKey,
    pub(crate) factory: Box<dyn StreamReceiverFactory>,
}

pub(crate) struct StreamIngestTask {
    join_handle: JoinHandle<()>,
    _stream_reader: StreamReader,
}

impl StreamIngestTask {
    pub(crate) fn new(
        mut receiver: mpsc::Receiver<StreamReceiveStart>,
        cache_store: Arc<TrackCacheStore>,
        sender_map: Arc<SenderMap>,
    ) -> Self {
        let (opened_tx, opened_rx) = mpsc::channel::<StreamOpened>(64);
        let stream_reader = StreamReader::run(opened_rx, cache_store, sender_map);

        let join_handle = tokio::spawn(async move {
            let mut joinset = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(cmd) = receiver.recv() => {
                        joinset.spawn(Self::factory_loop(
                            cmd.track_key,
                            cmd.factory,
                            opened_tx.clone(),
                        ));
                    }
                    Some(result) = joinset.join_next() => {
                        if let Err(e) = result {
                            tracing::error!("stream accept task panicked: {:?}", e);
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
    ) {
        loop {
            let receiver = match factory.next().await {
                Ok(r) => r,
                Err(_) => return,
            };
            if stream_tx
                .send(StreamOpened {
                    track_key,
                    receiver,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

impl Drop for StreamIngestTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

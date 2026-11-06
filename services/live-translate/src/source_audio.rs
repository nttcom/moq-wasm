use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use mediapack::loc::{LocObject, from_extension_headers};
use moqt::{Session, subscribe::SubscribeOption};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

use crate::opus_codec::OpusDecoder;

const SOURCE_PCM_CAPACITY: usize = 256;
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) struct SourcePcm {
    pub(crate) input_timestamp_micros: u64,
    pub(crate) samples: Arc<[i16]>,
}

pub(crate) struct SourceAudio {
    source_pcm_sender: broadcast::Sender<SourcePcm>,
    join_handle: JoinHandle<()>,
}

impl SourceAudio {
    pub(crate) fn run(
        session: Arc<Session>,
        namespace: String,
        track_name: String,
        channels: u32,
    ) -> Self {
        let (source_pcm_sender, _) = broadcast::channel(SOURCE_PCM_CAPACITY);
        let sender = source_pcm_sender.clone();
        // The subscription is renewed whenever it ends or fails, so the languages
        // keep their receivers and a refused SUBSCRIBE does not silence the
        // broadcast for good.
        let join_handle = tokio::spawn(async move {
            loop {
                match receive(&session, &namespace, &track_name, channels, &sender).await {
                    Ok(()) => tracing::info!(%namespace, %track_name, "source audio ended"),
                    Err(error) => {
                        tracing::warn!(%namespace, %track_name, ?error, "source audio stopped")
                    }
                }
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
            }
        });
        Self {
            source_pcm_sender,
            join_handle,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SourcePcm> {
        self.source_pcm_sender.subscribe()
    }
}

impl Drop for SourceAudio {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn receive(
    session: &Session,
    namespace: &str,
    track_name: &str,
    channels: u32,
    source_pcm_sender: &broadcast::Sender<SourcePcm>,
) -> Result<()> {
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            namespace.to_string(),
            track_name.to_string(),
            SubscribeOption::default(),
        )
        .await
        .context("subscribe to the source audio")?;
    tracing::info!(%namespace, %track_name, "subscribed to source audio");
    let result = decode(&mut subscriber, &subscription, channels, source_pcm_sender).await;
    let _ = subscriber.unsubscribe(&subscription).await;
    result
}

async fn decode(
    subscriber: &mut moqt::Subscriber,
    subscription: &moqt::Subscription,
    channels: u32,
    source_pcm_sender: &broadcast::Sender<SourcePcm>,
) -> Result<()> {
    let mut reader = subscriber.track_reader(subscription);
    let (pcm_sender, mut pcm_receiver) = mpsc::unbounded_channel();
    let decoder = OpusDecoder::new(channels, pcm_sender)?;
    let mut latest_input_timestamp_micros = None;
    loop {
        tokio::select! {
            object = reader.next_object() => {
                let Some(object) = object? else {
                    return Ok(());
                };
                let loc = LocObject {
                    extensions: from_extension_headers(&object.extension_headers),
                    payload: object.payload,
                };
                let Some(timestamp) = loc.capture_timestamp() else {
                    tracing::warn!("source audio object without a capture timestamp");
                    continue;
                };
                latest_input_timestamp_micros = Some(timestamp.micros());
                decoder.push(&loc.payload)?;
            }
            Some(samples) = pcm_receiver.recv() => {
                if let Some(input_timestamp_micros) = latest_input_timestamp_micros {
                    let _ = source_pcm_sender.send(SourcePcm {
                        input_timestamp_micros,
                        samples: samples.into(),
                    });
                }
            }
        }
    }
}

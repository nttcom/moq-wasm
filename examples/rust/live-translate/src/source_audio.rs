use std::{sync::Arc, time::Duration};

use anyhow::Result;
use moq_wasm::{
    Session,
    media::{FollowOptions, FollowedTrack, RemoteBroadcast, loc_timestamp_micros},
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

use crate::opus_codec::OpusDecoder;

const SOURCE_PCM_CAPACITY: usize = 256;
/// The relay drops source audio it has not sent this long after receiving it
/// (draft-14 §9.2.1.2), so the translation catches up instead of falling
/// behind. Longer than a player's 2 s because a dropped stretch loses words.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The SDK subscribes again whenever the subscription ends or fails, so
    /// the languages keep their receivers and a refused SUBSCRIBE does not
    /// silence the broadcast for good.
    pub(crate) fn run(
        session: &Session,
        source: &RemoteBroadcast,
        track_name: &str,
        channels: u32,
    ) -> Self {
        let (source_pcm_sender, _) = broadcast::channel(SOURCE_PCM_CAPACITY);
        let followed = source.follow(
            track_name,
            FollowOptions {
                delivery_timeout: Some(DELIVERY_TIMEOUT),
                ..FollowOptions::default()
            },
        );
        let track_namespace = source.namespace().join("/");
        let track_name = track_name.to_string();
        let sender = source_pcm_sender.clone();
        let join_handle = tokio::spawn(session.scoped(async move {
            tracing::info!(track_namespace, track_name, "following source audio");
            if let Err(error) = decode(followed, channels, &sender).await {
                tracing::warn!(track_namespace, track_name, ?error, "source audio stopped");
            }
        }));
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

async fn decode(
    mut followed: FollowedTrack,
    channels: u32,
    source_pcm_sender: &broadcast::Sender<SourcePcm>,
) -> Result<()> {
    let (pcm_sender, mut pcm_receiver) = mpsc::unbounded_channel();
    let decoder = OpusDecoder::new(channels, pcm_sender)?;
    let mut latest_input_timestamp_micros = None;
    loop {
        tokio::select! {
            object = followed.next() => {
                let Some(timestamp_micros) = loc_timestamp_micros(&object.extension_headers) else {
                    tracing::warn!("source audio object without a capture timestamp");
                    continue;
                };
                latest_input_timestamp_micros = Some(timestamp_micros);
                decoder.push(&object.payload)?;
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

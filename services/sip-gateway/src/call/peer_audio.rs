use std::{sync::Arc, time::Duration};

use anyhow::Result;
use broadcast::BroadcastReader;
use bytes::Bytes;
use mediapack::{AudioConfig, MediaEvent};
use moqt::client;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::MissedTickBehavior,
};

use crate::media::{g711::G711, playout::Playout, rtp_leg::RtpSender, transcoder::Transcoder};

/// Opus waiting for the call to be answered is dropped beyond this.
const PEER_AUDIO_CAPACITY: usize = 64;
const RETRY_DELAY: Duration = Duration::from_secs(1);
const PACKET_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) enum PeerAudio {
    Channels(u8),
    Packet(Bytes),
}

/// Waits for the peer's broadcast, reads its audio and reports when the
/// peer ends it (PUBLISH_NAMESPACE_DONE).
pub(crate) struct PeerWatchTask {
    join_handle: JoinHandle<()>,
}

impl PeerWatchTask {
    pub(crate) fn run(
        session: Arc<client::Session>,
        peer: Vec<String>,
    ) -> (Self, mpsc::Receiver<PeerAudio>, oneshot::Receiver<()>) {
        let (audio_sender, audio_receiver) = mpsc::channel(PEER_AUDIO_CAPACITY);
        let (left_sender, left_receiver) = oneshot::channel();
        let join_handle = tokio::spawn(async move {
            match watch(&session, &peer, &audio_sender).await {
                Ok(()) => {
                    tracing::info!(track_namespace = peer.join("/"), "peer left the call");
                    let _ = left_sender.send(());
                }
                Err(error) => tracing::warn!(
                    %error,
                    track_namespace = peer.join("/"),
                    "peer not watched"
                ),
            }
        });
        (Self { join_handle }, audio_receiver, left_receiver)
    }
}

impl Drop for PeerWatchTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn watch(
    session: &client::Session,
    peer: &[String],
    audio_sender: &mpsc::Sender<PeerAudio>,
) -> Result<()> {
    let namespaces = session.watch_namespaces(peer).await?;
    let namespace = peer.join("/");
    loop {
        match namespaces.next().await {
            Some(event) if event.namespace == peer && event.active => break,
            Some(_) => {}
            None => anyhow::bail!("the relay session ended"),
        }
    }
    loop {
        let forwarding = forward(session, &namespace, audio_sender);
        tokio::pin!(forwarding);
        loop {
            tokio::select! {
                event = namespaces.next() => match event {
                    Some(event) if event.namespace == peer && !event.active => return Ok(()),
                    Some(_) => {}
                    None => anyhow::bail!("the relay session ended"),
                },
                result = &mut forwarding => {
                    if let Err(error) = result {
                        tracing::warn!(%error, track_namespace = %namespace, "peer audio not read");
                    }
                    break;
                }
            }
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

async fn forward(
    session: &client::Session,
    namespace: &str,
    audio_sender: &mpsc::Sender<PeerAudio>,
) -> Result<()> {
    let mut reader = BroadcastReader::run(session.moqt_session(), namespace).await?;
    tracing::info!(track_namespace = namespace, "peer audio subscribed");
    while let Some(event) = reader.next_event().await {
        let audio = match event {
            MediaEvent::AudioConfig(AudioConfig::Opus { channels, .. }) => {
                PeerAudio::Channels(channels)
            }
            MediaEvent::AudioConfig(config) => {
                anyhow::bail!("{} audio is not supported", config.codec_string())
            }
            MediaEvent::Audio(sample) => PeerAudio::Packet(sample.data),
            _ => continue,
        };
        // Full only before the answer: that audio would be stale anyway.
        let _ = audio_sender.try_send(audio);
    }
    Ok(())
}

/// Sends the peer's audio to the phone as one G.711 packet every 20 ms,
/// silence while there is none.
pub(crate) struct PlayoutTask {
    join_handle: JoinHandle<()>,
}

impl PlayoutTask {
    pub(crate) fn run(
        codec: G711,
        mut sender: RtpSender,
        mut audio_receiver: mpsc::Receiver<PeerAudio>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let (g711_sender, mut g711_receiver) = mpsc::unbounded_channel::<Vec<u8>>();
            let mut transcoder = None;
            let mut playout = Playout::new(codec);
            let mut interval = tokio::time::interval(PACKET_INTERVAL);
            // A late tick is sent at once, so the RTP clock keeps up with the
            // wall clock.
            interval.set_missed_tick_behavior(MissedTickBehavior::Burst);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(error) = sender.send(playout.next_packet()).await {
                            tracing::warn!(%error, "audio to the phone dropped");
                        }
                    }
                    Some(encoded) = g711_receiver.recv() => playout.push(&encoded),
                    Some(audio) = audio_receiver.recv() => match audio {
                        PeerAudio::Channels(channels) => {
                            transcoder = Transcoder::opus_to_g711(codec, channels, g711_sender.clone())
                                .inspect_err(|error| tracing::error!(%error, "peer audio not transcoded"))
                                .ok();
                        }
                        PeerAudio::Packet(packet) => {
                            if let Some(transcoder) = &transcoder
                                && let Err(error) = transcoder.push(&packet)
                            {
                                tracing::warn!(%error, "peer audio dropped");
                            }
                        }
                    },
                }
            }
        });
        Self { join_handle }
    }
}

impl Drop for PlayoutTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

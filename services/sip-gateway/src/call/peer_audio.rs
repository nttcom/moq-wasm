use std::{sync::Arc, time::Duration};

use anyhow::Result;
use broadcast::{
    BroadcastReader,
    selection::{TrackChoice, VideoChoice},
};
use bytes::Bytes;
use mediapack::{AudioConfig, MediaEvent};
use moqt::client;
use tokio::{sync::mpsc, task::JoinHandle, time::MissedTickBehavior};

use super::peer_control::{Command, read_commands};
use crate::media::{
    dtmf::DtmfTone,
    playout::Playout,
    rtp_leg::{Negotiated, RtpSender},
    transcoder::Transcoder,
};

/// Opus waiting for the call to be answered is dropped beyond this.
const PEER_AUDIO_CAPACITY: usize = 64;
const RETRY_DELAY: Duration = Duration::from_secs(1);
const COMMAND_CAPACITY: usize = 8;
const PACKET_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) enum PeerAudio {
    Channels(u8),
    Packet(Bytes),
}

/// Waits for the peer's broadcast and reads its audio and its commands; the
/// peer ending its broadcast (PUBLISH_NAMESPACE_DONE) is a `Command::Hangup`.
pub(crate) struct PeerWatchTask {
    join_handle: JoinHandle<()>,
}

impl PeerWatchTask {
    pub(crate) fn run(
        session: Arc<client::Session>,
        peer: Vec<String>,
    ) -> (Self, mpsc::Receiver<PeerAudio>, mpsc::Receiver<Command>) {
        let (audio_sender, audio_receiver) = mpsc::channel(PEER_AUDIO_CAPACITY);
        let (command_sender, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let join_handle = tokio::spawn(async move {
            match watch(&session, &peer, &audio_sender, &command_sender).await {
                Ok(()) => {
                    tracing::info!(track_namespace = peer.join("/"), "peer left the call");
                    let _ = command_sender.send(Command::Hangup).await;
                }
                Err(error) => tracing::warn!(
                    %error,
                    track_namespace = peer.join("/"),
                    "peer not watched"
                ),
            }
        });
        (Self { join_handle }, audio_receiver, command_receiver)
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
    command_sender: &mpsc::Sender<Command>,
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
        let commanding = read_commands_until_left(session, peer, command_sender);
        tokio::pin!(commanding);
        loop {
            tokio::select! {
                () = &mut commanding => {}
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

/// Reads the peer's commands, subscribing again `RETRY_DELAY` after each
/// failure or end of the `control` track, which the page may publish after
/// its namespace. Never returns; `watch` drops it once the peer leaves.
async fn read_commands_until_left(
    session: &client::Session,
    peer: &[String],
    command_sender: &mpsc::Sender<Command>,
) {
    loop {
        if let Err(error) = read_commands(session, peer, command_sender).await {
            tracing::debug!(%error, track_namespace = peer.join("/"), "peer commands not read");
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

async fn forward(
    session: &client::Session,
    namespace: &str,
    audio_sender: &mpsc::Sender<PeerAudio>,
) -> Result<()> {
    // A meeting participant encodes a video rendition only while it has a
    // subscriber, so the phone's call does not subscribe to one.
    let choice = TrackChoice {
        video: VideoChoice::Off,
        ..TrackChoice::default()
    };
    let mut reader = BroadcastReader::run(session.moqt_session(), namespace, choice).await?;
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
/// silence while there is none, and the peer's DTMF digits as RFC 4733
/// events in the place of the audio while each lasts.
pub(crate) struct PlayoutTask {
    join_handle: JoinHandle<()>,
}

impl PlayoutTask {
    pub(crate) fn run(
        call_id: String,
        negotiated: Negotiated,
        mut sender: RtpSender,
        mut audio_receiver: mpsc::Receiver<PeerAudio>,
        mut digit_receiver: mpsc::UnboundedReceiver<char>,
    ) -> Self {
        let codec = negotiated.codec;
        let join_handle = tokio::spawn(async move {
            let mut tone: Option<DtmfTone> = None;
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
                        // The audio of the tick an event takes is dropped.
                        let audio = playout.next_packet();
                        if tone.is_none()
                            && let Ok(digit) = digit_receiver.try_recv()
                        {
                            tone = start_tone(&call_id, negotiated.telephone_event, digit);
                        }
                        let event = negotiated
                            .telephone_event
                            .zip(tone.as_mut().and_then(DtmfTone::next_packet));
                        let sent = match event {
                            Some((payload_type, packet)) => sender.send_event(payload_type, packet).await,
                            None => {
                                tone = None;
                                sender.send(audio).await
                            }
                        };
                        if let Err(error) = sent {
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

/// The tone of `digit`, unless the trunk accepts no RFC 4733 events.
fn start_tone(call_id: &str, telephone_event: Option<u8>, digit: char) -> Option<DtmfTone> {
    if telephone_event.is_none() {
        tracing::warn!(
            call_id,
            "DTMF dropped: the trunk accepts no telephone-event"
        );
        return None;
    }
    tracing::info!(call_id, "DTMF tone started");
    DtmfTone::new(digit)
}

impl Drop for PlayoutTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

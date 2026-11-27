use std::{collections::VecDeque, sync::Arc};

use anyhow::Result;
use bytes::Bytes;
use moq_wasm::{Session, TrackWriter, media::loc_timestamp_headers};
use tokio::{
    sync::{OwnedSemaphorePermit, broadcast, mpsc},
    task::JoinHandle,
};

use crate::{
    opus_codec::{GEMINI_OUTPUT_SAMPLE_RATE, OpusEncoder},
    source_audio::SourcePcm,
    target_language::TargetLanguage,
    translated_audio::{TranslatedAudioSequencer, TranslatedFrame},
    translation_engine::{RunningTranslation, Transcript, TranslationEngine, TranslationOutput},
};

const GROUP_DURATION_MICROS: u64 = 2_000_000;
const ENGINE_CHANNEL_CAPACITY: usize = 64;

pub(crate) struct LanguageTranslation {
    source_text_sender: mpsc::UnboundedSender<Option<Arc<TrackWriter>>>,
    join_handle: JoinHandle<()>,
}

/// The language's own tracks, which it writes for as long as it runs.
pub(crate) struct LanguageWriters {
    pub(crate) audio: Arc<TrackWriter>,
    pub(crate) text: Arc<TrackWriter>,
}

impl LanguageTranslation {
    /// `ended_sender` gets the language's code when its engine gives up, so
    /// the owner ends the language's tracks.
    pub(crate) fn run(
        session: &Session,
        engine: &TranslationEngine,
        language: &'static TargetLanguage,
        writers: LanguageWriters,
        source_pcm_receiver: broadcast::Receiver<SourcePcm>,
        capacity_permit: OwnedSemaphorePermit,
        ended_sender: mpsc::UnboundedSender<&'static str>,
    ) -> Result<Self> {
        let (pcm_sender, pcm_receiver) = mpsc::channel(ENGINE_CHANNEL_CAPACITY);
        let (translated_sender, translated_receiver) = mpsc::channel(ENGINE_CHANNEL_CAPACITY);
        let (packet_sender, packet_receiver) = mpsc::unbounded_channel();
        let (source_text_sender, source_text_receiver) = mpsc::unbounded_channel();
        let translation = Translation {
            language,
            encoder: OpusEncoder::new(packet_sender)?,
            sequencer: TranslatedAudioSequencer::new(
                language.gemini_delay,
                GEMINI_OUTPUT_SAMPLE_RATE,
            ),
            pending_timestamps: VecDeque::new(),
            pcm_sender,
            audio: Writer::new(writers.audio),
            text: Writer::new(writers.text),
            source_text: None,
        };
        let running = engine.start(language, pcm_receiver, translated_sender);
        let join_handle = tokio::spawn(session.scoped(translation.translate(
            running,
            capacity_permit,
            Channels {
                source_pcm_receiver,
                translated_receiver,
                packet_receiver,
                source_text_receiver,
            },
            ended_sender,
        )));
        Ok(Self {
            source_text_sender,
            join_handle,
        })
    }

    /// Has the language also write the source transcript (`Some`), or stop
    /// writing it (`None`).
    pub(crate) fn set_source_text(&self, writer: Option<Arc<TrackWriter>>) {
        let _ = self.source_text_sender.send(writer);
    }
}

impl Drop for LanguageTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

struct Channels {
    source_pcm_receiver: broadcast::Receiver<SourcePcm>,
    translated_receiver: mpsc::Receiver<TranslationOutput>,
    packet_receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    source_text_receiver: mpsc::UnboundedReceiver<Option<Arc<TrackWriter>>>,
}

struct Translation {
    language: &'static TargetLanguage,
    encoder: OpusEncoder,
    sequencer: TranslatedAudioSequencer,
    pending_timestamps: VecDeque<u64>,
    pcm_sender: mpsc::Sender<Vec<i16>>,
    audio: Writer,
    text: Writer,
    source_text: Option<Writer>,
}

impl Translation {
    async fn translate(
        mut self,
        _running: RunningTranslation,
        _capacity_permit: OwnedSemaphorePermit,
        mut channels: Channels,
        ended_sender: mpsc::UnboundedSender<&'static str>,
    ) {
        tracing::info!(language = self.language.code, "translation started");
        loop {
            tokio::select! {
                source = channels.source_pcm_receiver.recv() => match source {
                    Ok(source) => self.forward_source(source),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(language = self.language.code, skipped, "source audio lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                output = channels.translated_receiver.recv() => match output {
                    None => {
                        tracing::warn!(language = self.language.code, "translation engine gave up");
                        let _ = ended_sender.send(self.language.code);
                        return;
                    }
                    Some(TranslationOutput::ConnectionStarted) => {
                        let frames = self.sequencer.restart();
                        self.encode(frames);
                    }
                    Some(TranslationOutput::Audio(samples)) => {
                        tracing::debug!(
                            language = self.language.code,
                            samples = samples.len(),
                            "translated chunk received"
                        );
                        let frames = self.sequencer.push_chunk(&samples);
                        self.encode(frames);
                    }
                    Some(TranslationOutput::TranslatedTranscript(transcript)) => {
                        // Spoken at the point the translated audio has reached.
                        let timestamp = self.sequencer.translated_timestamp_micros();
                        self.publish_text(Track::Translated, timestamp, transcript).await;
                    }
                    Some(TranslationOutput::SourceTranscript(transcript)) => {
                        // Heard up to the newest source audio sent.
                        let timestamp = self.sequencer.input_timestamp_micros();
                        self.publish_text(Track::Source, timestamp, transcript).await;
                    }
                },
                Some(packet) = channels.packet_receiver.recv() => self.publish(packet).await,
                Some(writer) = channels.source_text_receiver.recv() => {
                    self.source_text = writer.map(Writer::new);
                }
                else => return,
            }
        }
    }

    fn forward_source(&mut self, source: SourcePcm) {
        self.sequencer.record_input(source.input_timestamp_micros);
        if self.pcm_sender.try_send(source.samples.to_vec()).is_err() {
            tracing::debug!(
                language = self.language.code,
                "translation input full, dropping audio"
            );
        }
    }

    fn encode(&mut self, frames: Vec<TranslatedFrame>) {
        for frame in frames {
            match self.encoder.push(&frame.samples) {
                Ok(()) => self.pending_timestamps.push_back(frame.timestamp_micros),
                Err(error) => {
                    tracing::warn!(language = self.language.code, ?error, "Opus encode failed")
                }
            }
        }
    }

    async fn publish(&mut self, packet: Vec<u8>) {
        let Some(timestamp_micros) = self.pending_timestamps.pop_front() else {
            tracing::warn!(
                language = self.language.code,
                "Opus packet without a frame timestamp"
            );
            return;
        };
        if let Err(error) = self
            .audio
            .write(timestamp_micros, Bytes::from(packet))
            .await
        {
            tracing::warn!(
                language = self.language.code,
                ?error,
                "translated audio write failed"
            );
        }
    }

    /// Stamped like the audio, so the viewer shows it when that audio plays.
    async fn publish_text(
        &mut self,
        track: Track,
        timestamp_micros: Option<u64>,
        transcript: Transcript,
    ) {
        let writer = match track {
            Track::Translated => Some(&mut self.text),
            Track::Source => self.source_text.as_mut(),
        };
        let (Some(writer), Some(timestamp_micros)) = (writer, timestamp_micros) else {
            return;
        };
        let payload = serde_json::json!({ "text": transcript.text, "final": transcript.finished });
        if let Err(error) = writer
            .write(timestamp_micros, Bytes::from(payload.to_string()))
            .await
        {
            tracing::warn!(
                language = self.language.code,
                ?error,
                "transcript write failed"
            );
        }
    }
}

enum Track {
    Translated,
    Source,
}

/// A track written in groups of `GROUP_DURATION_MICROS` of media time, each
/// object carrying its LOC Timestamp.
struct Writer {
    track: Arc<TrackWriter>,
    group: Option<u64>,
}

impl Writer {
    fn new(track: Arc<TrackWriter>) -> Self {
        Self { track, group: None }
    }

    async fn write(&mut self, timestamp_micros: u64, payload: Bytes) -> Result<()> {
        let group = timestamp_micros / GROUP_DURATION_MICROS;
        let new_group = self.group != Some(group);
        self.group = Some(group);
        self.track
            .write(payload, new_group, loc_timestamp_headers(timestamp_micros))
            .await?;
        Ok(())
    }
}

use std::collections::VecDeque;

use anyhow::Result;
use bytes::Bytes;
use mediapack::loc::{LocExtension, TIMESTAMP_ID, to_extension_headers};
use moqt::TrackWriter;
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
    writer_sender: mpsc::UnboundedSender<WriterUpdate>,
    join_handle: JoinHandle<()>,
}

/// Which of the language's tracks a subscription opened (`Some`) or closed
/// (`None`).
pub(crate) enum WriterUpdate {
    Audio(Option<TrackWriter>),
    Text(Option<TrackWriter>),
    SourceText(Option<TrackWriter>),
}

impl LanguageTranslation {
    /// `ended_sender` gets the language's code when its engine gives up, so
    /// the owner ends the language's subscriptions.
    pub(crate) fn run(
        engine: &TranslationEngine,
        language: &'static TargetLanguage,
        source_pcm_receiver: broadcast::Receiver<SourcePcm>,
        capacity_permit: OwnedSemaphorePermit,
        ended_sender: mpsc::UnboundedSender<&'static str>,
    ) -> Result<Self> {
        let (pcm_sender, pcm_receiver) = mpsc::channel(ENGINE_CHANNEL_CAPACITY);
        let (translated_sender, translated_receiver) = mpsc::channel(ENGINE_CHANNEL_CAPACITY);
        let (packet_sender, packet_receiver) = mpsc::unbounded_channel();
        let (writer_sender, writer_receiver) = mpsc::unbounded_channel();
        let translation = Translation {
            language,
            encoder: OpusEncoder::new(packet_sender)?,
            sequencer: TranslatedAudioSequencer::new(
                language.gemini_delay,
                GEMINI_OUTPUT_SAMPLE_RATE,
            ),
            pending_timestamps: VecDeque::new(),
            pcm_sender,
            writer: None,
            text_writer: None,
            source_text_writer: None,
        };
        let running = engine.start(language, pcm_receiver, translated_sender);
        let join_handle = tokio::spawn(translation.translate(
            running,
            capacity_permit,
            Channels {
                source_pcm_receiver,
                translated_receiver,
                packet_receiver,
                writer_receiver,
            },
            ended_sender,
        ));
        Ok(Self {
            writer_sender,
            join_handle,
        })
    }

    pub(crate) fn replace_writer(&self, update: WriterUpdate) {
        let _ = self.writer_sender.send(update);
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
    writer_receiver: mpsc::UnboundedReceiver<WriterUpdate>,
}

struct Translation {
    language: &'static TargetLanguage,
    encoder: OpusEncoder,
    sequencer: TranslatedAudioSequencer,
    pending_timestamps: VecDeque<u64>,
    pcm_sender: mpsc::Sender<Vec<i16>>,
    writer: Option<TrackWriter>,
    text_writer: Option<TrackWriter>,
    source_text_writer: Option<TrackWriter>,
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
                Some(update) = channels.writer_receiver.recv() => match update {
                    WriterUpdate::Audio(writer) => self.writer = writer,
                    WriterUpdate::Text(writer) => self.text_writer = writer,
                    WriterUpdate::SourceText(writer) => self.source_text_writer = writer,
                },
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
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if let Err(error) = write_object(writer, timestamp_micros, Bytes::from(packet)).await {
            tracing::warn!(
                language = self.language.code,
                ?error,
                "translated audio write failed"
            );
            self.writer = None;
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
            Track::Translated => &mut self.text_writer,
            Track::Source => &mut self.source_text_writer,
        };
        let (Some(writer_ref), Some(timestamp_micros)) = (writer.as_mut(), timestamp_micros) else {
            return;
        };
        let payload = serde_json::json!({ "text": transcript.text, "final": transcript.finished });
        if let Err(error) = write_object(
            writer_ref,
            timestamp_micros,
            Bytes::from(payload.to_string()),
        )
        .await
        {
            tracing::warn!(
                language = self.language.code,
                ?error,
                "transcript write failed"
            );
            *writer = None;
        }
    }
}

enum Track {
    Translated,
    Source,
}

async fn write_object(
    writer: &mut TrackWriter,
    timestamp_micros: u64,
    payload: Bytes,
) -> Result<()> {
    let group_id = timestamp_micros / GROUP_DURATION_MICROS;
    if writer.current_group_id() != Some(group_id) {
        writer.start_group_at(group_id).await?;
    }
    let capture_timestamp = LocExtension::varint(TIMESTAMP_ID, timestamp_micros);
    writer
        .write_with_extension_headers(payload, to_extension_headers(&[capture_timestamp]))
        .await
}

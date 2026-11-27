use std::{collections::HashMap, future::pending, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use moq_wasm::{
    PublishTrackOptions, Session, TrackWriter,
    media::{Catalog, RemoteBroadcast, create_broadcast},
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    broadcast_namespace::TRANSLATED_SUFFIX,
    catalog_track::CatalogTrack,
    language_translation::{LanguageTranslation, LanguageWriters},
    root_translation::{TranslationPolicy, TranslationResources},
    source_audio::SourceAudio,
    source_catalog::SourceCatalog,
    target_language::{SOURCE_TEXT_TRACK, TARGET_LANGUAGES, TargetLanguage, find_by_code},
    translated_catalog::{build_translated_catalog, source_audio_track},
    translation_engine::TranslationEngine,
};

const CATALOG_TRACK_NAME: &str = "catalog";
const LANGUAGE_IDLE_GRACE: Duration = Duration::from_secs(5);
const POLICY_CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Covers the source catalog's SUBSCRIBE retries and the wait for the audio
/// track to be listed.
const FIRST_CATALOG_TIMEOUT: Duration = Duration::from_secs(30);

enum StartRefusal {
    Capacity,
    Failed(anyhow::Error),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TrackKey {
    Audio(&'static TargetLanguage),
    Text(&'static TargetLanguage),
    SourceText,
}

impl TrackKey {
    fn all() -> impl Iterator<Item = Self> {
        TARGET_LANGUAGES
            .iter()
            .flat_map(|language| [Self::Audio(language), Self::Text(language)])
            .chain([Self::SourceText])
    }

    fn name(self) -> String {
        match self {
            Self::Audio(language) => language.track_name(),
            Self::Text(language) => language.text_track_name(),
            Self::SourceText => SOURCE_TEXT_TRACK.to_string(),
        }
    }
}

/// One report of `TrackWriter::demand_change`. `generation` drops reports
/// still queued from a writer that `TranslatedTracks::end` replaced.
struct DemandChange {
    key: TrackKey,
    generation: u64,
    wanted: bool,
}

struct OnDemandTrack {
    writer: Arc<TrackWriter>,
    generation: u64,
    wanted: bool,
    demand_watch: JoinHandle<()>,
}

impl OnDemandTrack {
    fn watch(
        session: &Session,
        key: TrackKey,
        writer: TrackWriter,
        generation: u64,
        demand_sender: mpsc::UnboundedSender<DemandChange>,
    ) -> Self {
        let writer = Arc::new(writer);
        let demand_watch = tokio::spawn(session.scoped({
            let writer = writer.clone();
            async move {
                while let Ok(wanted) = writer.demand_change().await {
                    let change = DemandChange {
                        key,
                        generation,
                        wanted,
                    };
                    if demand_sender.send(change).is_err() {
                        return;
                    }
                }
            }
        }));
        Self {
            writer,
            generation,
            wanted: false,
            demand_watch,
        }
    }
}

impl Drop for OnDemandTrack {
    fn drop(&mut self) {
        self.demand_watch.abort();
    }
}

struct LanguageSlot {
    translation: LanguageTranslation,
    idle_since: Option<Instant>,
    started_at: Instant,
    /// An always-on language: neither viewers leaving nor the time limit stop it.
    pinned: bool,
}

/// Publishes `{broadcast}/translated` for one source broadcast until the
/// source withdraws its namespace, which aborts the task.
pub(crate) async fn serve(
    session: Arc<Session>,
    resources: TranslationResources,
    source: RemoteBroadcast,
) -> Result<()> {
    let broadcast_namespace = source.namespace().join("/");
    let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel();
    let _source_catalog = SourceCatalog::run(&session, source.clone(), catalog_sender);
    let (source_catalog, translated_catalog) = tokio::time::timeout(
        FIRST_CATALOG_TIMEOUT,
        first_translatable_catalog(&mut catalog_receiver, &broadcast_namespace),
    )
    .await
    .context("source catalog did not list Opus audio in time")??;
    let source_track = source_audio_track(&source_catalog)
        .context("source catalog has no Opus audio to translate")?;
    let source_track_name = source_track.name.clone();
    let source_channels = channel_count(source_track.channel_config.as_deref());

    let translated_namespace: Vec<String> = source
        .namespace()
        .iter()
        .cloned()
        .chain([TRANSLATED_SUFFIX.to_string()])
        .collect();
    // Every track is registered before PUBLISH_NAMESPACE, so a SUBSCRIBE that
    // follows it at once finds its track.
    let builder = create_broadcast(&session, &translated_namespace);
    let catalog_writer = builder
        .publish_track(CATALOG_TRACK_NAME, PublishTrackOptions::default())
        .await?;
    let (demand_sender, mut demand_receiver) = mpsc::unbounded_channel();
    let mut tracks = HashMap::new();
    for key in TrackKey::all() {
        let writer = builder.publish_track(&key.name(), on_demand()).await?;
        let track = OnDemandTrack::watch(&session, key, writer, 0, demand_sender.clone());
        tracks.insert(key, track);
    }
    let broadcast = builder
        .announce()
        .await
        .context("announce the translated namespace")?;
    let (language_ended_sender, mut language_ended_receiver) = mpsc::unbounded_channel();
    let TranslationResources {
        engine,
        capacity,
        policy,
    } = resources;
    let mut serving = Serving {
        catalog: CatalogTrack::run(&session, catalog_writer, translated_catalog.to_bytes()),
        session,
        engine,
        capacity,
        policy,
        source,
        broadcast_namespace,
        source_track_name,
        source_channels,
        translated: TranslatedTracks {
            broadcast,
            tracks,
            next_generation: 1,
            demand_sender,
        },
        languages: HashMap::new(),
        source_text_language: None,
        source_audio: None,
        language_ended_sender,
    };
    tracing::info!(
        track_namespace = translated_namespace.join("/"),
        source_track = %serving.source_track_name,
        "translation ready"
    );
    serving.start_always_on();
    let mut policy_check = tokio::time::interval(POLICY_CHECK_INTERVAL);
    loop {
        let idle_deadline = serving.earliest_idle_deadline();
        tokio::select! {
            _ = policy_check.tick() => {
                serving.enforce_limits().await;
                serving.start_always_on();
            }
            Some(change) = demand_receiver.recv() => serving.demand_changed(change).await,
            Some(catalog) = catalog_receiver.recv() => serving.update_catalog(&catalog),
            Some(code) = language_ended_receiver.recv() => {
                serving.end_language(code, "translation unavailable").await;
            }
            _ = sleep_until(idle_deadline) => serving.stop_idle_languages().await,
        }
    }
}

fn on_demand() -> PublishTrackOptions {
    PublishTrackOptions {
        on_demand: true,
        ..PublishTrackOptions::default()
    }
}

/// The translated namespace and its on-demand tracks.
struct TranslatedTracks {
    broadcast: moq_wasm::media::LocalBroadcast,
    tracks: HashMap<TrackKey, OnDemandTrack>,
    next_generation: u64,
    demand_sender: mpsc::UnboundedSender<DemandChange>,
}

impl TranslatedTracks {
    fn wanted(&self, key: TrackKey) -> bool {
        self.tracks.get(&key).is_some_and(|track| track.wanted)
    }

    fn writer(&self, key: TrackKey) -> Option<Arc<TrackWriter>> {
        self.tracks.get(&key).map(|track| track.writer.clone())
    }

    /// Ends each of `keys` a subscriber wants with PUBLISH_DONE and publishes
    /// it again, so a viewer can subscribe anew. Viewers of an ended
    /// `audio_<lang>` fall back to the original audio.
    async fn end(&mut self, session: &Session, keys: &[TrackKey], reason: &str) {
        for &key in keys {
            if !self.wanted(key) {
                continue;
            }
            let track_name = key.name();
            if let Some(track) = self.tracks.remove(&key)
                && let Err(error) = track.writer.end().await
            {
                tracing::warn!(track_name, ?error, "PUBLISH_DONE failed");
            }
            tracing::info!(track_name, reason, "translated track ended");
            match self.broadcast.publish_track(&track_name, on_demand()).await {
                Ok(writer) => {
                    let generation = self.next_generation;
                    self.next_generation += 1;
                    let track = OnDemandTrack::watch(
                        session,
                        key,
                        writer,
                        generation,
                        self.demand_sender.clone(),
                    );
                    self.tracks.insert(key, track);
                }
                Err(error) => tracing::warn!(track_name, ?error, "publish the track again"),
            }
        }
    }
}

struct Serving {
    session: Arc<Session>,
    engine: TranslationEngine,
    capacity: Arc<Semaphore>,
    policy: Arc<TranslationPolicy>,
    source: RemoteBroadcast,
    broadcast_namespace: String,
    source_track_name: String,
    source_channels: u32,
    catalog: CatalogTrack,
    translated: TranslatedTracks,
    languages: HashMap<&'static str, LanguageSlot>,
    /// The language writing `text_ja`.
    source_text_language: Option<&'static str>,
    source_audio: Option<SourceAudio>,
    /// Languages whose engine gave up report their code here.
    language_ended_sender: mpsc::UnboundedSender<&'static str>,
}

impl Serving {
    async fn demand_changed(&mut self, change: DemandChange) {
        let Some(track) = self.translated.tracks.get_mut(&change.key) else {
            return;
        };
        if track.generation != change.generation || track.wanted == change.wanted {
            return;
        }
        track.wanted = change.wanted;
        match change.key {
            TrackKey::Audio(code) | TrackKey::Text(code) => {
                self.language_demand_changed(code).await
            }
            TrackKey::SourceText => self.source_text_demand_changed(change.wanted).await,
        }
    }

    /// A language runs while its audio or its transcript is wanted, and
    /// idles once neither is.
    async fn language_demand_changed(&mut self, language: &'static TargetLanguage) {
        let code = language.code;
        let keys = [TrackKey::Audio(language), TrackKey::Text(language)];
        if !keys.iter().any(|&key| self.translated.wanted(key)) {
            if let Some(slot) = self.languages.get_mut(code) {
                slot.idle_since.get_or_insert_with(Instant::now);
            }
            return;
        }
        match self.start_language(language, false) {
            Ok(()) => {
                if let Some(slot) = self.languages.get_mut(code) {
                    slot.idle_since = None;
                }
            }
            Err(refusal) => {
                let reason = match &refusal {
                    StartRefusal::Capacity => "translation capacity reached",
                    StartRefusal::Failed(error) => {
                        tracing::warn!(language = code, ?error, "translation failed to start");
                        "translation failed to start"
                    }
                };
                self.translated.end(&self.session, &keys, reason).await;
            }
        }
    }

    /// The source transcript comes with every Gemini session, so one running
    /// language writes it; without any, the track is ended rather than
    /// paying for a session only to transcribe. Its demand does not keep the
    /// language from idling.
    async fn source_text_demand_changed(&mut self, wanted: bool) {
        if !wanted {
            if let Some(slot) = self
                .source_text_language
                .take()
                .and_then(|code| self.languages.get(code))
            {
                slot.translation.set_source_text(None);
            }
            return;
        }
        let Some(code) = self
            .languages
            .iter()
            .find(|(_, slot)| slot.idle_since.is_none())
            .or_else(|| self.languages.iter().next())
            .map(|(code, _)| *code)
        else {
            self.translated
                .end(
                    &self.session,
                    &[TrackKey::SourceText],
                    "no translation is running to transcribe the source",
                )
                .await;
            return;
        };
        if let Some(previous) = self.source_text_language.replace(code)
            && previous != code
            && let Some(slot) = self.languages.get(previous)
        {
            slot.translation.set_source_text(None);
        }
        if let Some(slot) = self.languages.get(code) {
            slot.translation
                .set_source_text(self.translated.writer(TrackKey::SourceText));
        }
    }

    /// `pinned` marks an always-on language; a start for a language that
    /// already runs leaves its `pinned` as it is.
    fn start_language(
        &mut self,
        language: &'static TargetLanguage,
        pinned: bool,
    ) -> Result<(), StartRefusal> {
        if self.languages.contains_key(language.code) {
            return Ok(());
        }
        let (Some(audio), Some(text)) = (
            self.translated.writer(TrackKey::Audio(language)),
            self.translated.writer(TrackKey::Text(language)),
        ) else {
            return Err(StartRefusal::Failed(anyhow::anyhow!(
                "the language's tracks are not published"
            )));
        };
        let Ok(capacity_permit) = self.capacity.clone().try_acquire_owned() else {
            return Err(StartRefusal::Capacity);
        };
        let source_audio = self.source_audio.get_or_insert_with(|| {
            SourceAudio::run(
                &self.session,
                &self.source,
                &self.source_track_name,
                self.source_channels,
            )
        });
        let translation = LanguageTranslation::run(
            &self.session,
            &self.engine,
            language,
            LanguageWriters { audio, text },
            source_audio.subscribe(),
            capacity_permit,
            self.language_ended_sender.clone(),
        )
        .map_err(StartRefusal::Failed)?;
        self.languages.insert(
            language.code,
            LanguageSlot {
                translation,
                idle_since: None,
                started_at: Instant::now(),
                pinned,
            },
        );
        Ok(())
    }

    /// Also called at every policy check, so a refused start is retried.
    fn start_always_on(&mut self) {
        for language in self.policy.always_on.clone() {
            if self.languages.contains_key(language.code) {
                continue;
            }
            match self.start_language(language, true) {
                Ok(()) => {
                    tracing::info!(language = language.code, "translation started, always on")
                }
                Err(StartRefusal::Capacity) => {
                    tracing::warn!(
                        language = language.code,
                        "always-on translation waits for capacity"
                    )
                }
                Err(StartRefusal::Failed(error)) => {
                    tracing::warn!(
                        language = language.code,
                        ?error,
                        "always-on translation failed to start"
                    )
                }
            }
        }
    }

    /// A source catalog that lists more tracks, such as the video once its
    /// codec is known, replaces the translated one. The audio track being
    /// translated stays the one of the first catalog.
    fn update_catalog(&mut self, source: &Catalog) {
        let Some(translated) = build_translated_catalog(source, &self.broadcast_namespace) else {
            return;
        };
        if self.catalog.update(translated.to_bytes()) {
            tracing::info!(
                track_namespace = self.broadcast_namespace,
                "translated catalog updated"
            );
        }
    }

    /// Ends every unpinned language that has run for `language_time_limit`.
    async fn enforce_limits(&mut self) {
        let now = Instant::now();
        let ended: Vec<&'static str> = self
            .languages
            .iter()
            .filter(|(_, slot)| {
                !slot.pinned && now >= slot.started_at + self.policy.language_time_limit
            })
            .map(|(code, _)| *code)
            .collect();
        for code in ended {
            self.end_language(code, "time limit").await;
        }
    }

    fn earliest_idle_deadline(&self) -> Option<Instant> {
        self.languages
            .values()
            .filter(|slot| !slot.pinned)
            .filter_map(|slot| slot.idle_since)
            .min()
            .map(|idle_since| idle_since + LANGUAGE_IDLE_GRACE)
    }

    async fn stop_idle_languages(&mut self) {
        let now = Instant::now();
        let expired: Vec<&'static str> = self
            .languages
            .iter()
            .filter(|(_, slot)| {
                !slot.pinned
                    && slot
                        .idle_since
                        .is_some_and(|idle_since| idle_since + LANGUAGE_IDLE_GRACE <= now)
            })
            .map(|(code, _)| *code)
            .collect();
        for code in expired {
            self.end_language(code, "no viewers").await;
        }
    }

    /// Drops the language, which stops its Gemini session, and ends its
    /// tracks, `text_ja` included when this language wrote it.
    async fn end_language(&mut self, code: &'static str, reason: &str) {
        let (Some(language), Some(slot)) = (find_by_code(code), self.languages.remove(code)) else {
            return;
        };
        let ran_minutes = slot.started_at.elapsed().as_secs() / 60;
        // Stops the language's writes before its tracks end and are published again.
        drop(slot);
        let mut keys = vec![TrackKey::Audio(language), TrackKey::Text(language)];
        if self.source_text_language == Some(code) {
            self.source_text_language = None;
            keys.push(TrackKey::SourceText);
        }
        self.translated.end(&self.session, &keys, reason).await;
        tracing::info!(language = code, reason, ran_minutes, "translation stopped");
        if self.languages.is_empty() {
            self.source_audio = None;
        }
    }
}

pub(crate) async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending().await,
    }
}

/// Waits for the first source catalog that lists Opus audio and returns it
/// with the translated catalog built from it.
async fn first_translatable_catalog(
    catalog_receiver: &mut mpsc::UnboundedReceiver<Catalog>,
    broadcast_namespace: &str,
) -> Result<(Catalog, Catalog)> {
    while let Some(source) = catalog_receiver.recv().await {
        if let Some(translated) = build_translated_catalog(&source, broadcast_namespace) {
            return Ok((source, translated));
        }
        tracing::debug!(
            track_namespace = broadcast_namespace,
            "source catalog lists no Opus audio yet"
        );
    }
    anyhow::bail!("source catalog ended before listing Opus audio")
}

/// The channel count of a `channelConfig` (draft-ietf-moq-msf-02 §5.2.30),
/// whose format the draft leaves open: `mono`, `stereo`, `<n>ch` or `<n>`;
/// one channel when absent or unknown.
fn channel_count(channel_config: Option<&str>) -> u32 {
    let count = match channel_config {
        Some("mono") | None => Some(1),
        Some("stereo") => Some(2),
        Some(other) => other.trim_end_matches("ch").parse().ok(),
    };
    count.filter(|&count| count > 0).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target_language::TARGET_LANGUAGES;

    fn catalog(json: &str) -> Catalog {
        Catalog::parse(json.as_bytes()).expect("catalog")
    }

    #[tokio::test]
    async fn the_first_catalog_with_opus_audio_starts_the_translation() {
        // Arrange: the publisher lists the video first and the audio with its next catalog
        let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel();
        let video_only = catalog(
            "{\"version\":\"draft-02\",\"tracks\":[{\"name\":\"video\",\"packaging\":\"loc\",\"role\":\"video\",\"isLive\":true,\"codec\":\"vp8\",\"bitrate\":1000000,\"framerate\":30}]}",
        );
        let with_audio = catalog(
            "{\"version\":\"draft-02\",\"tracks\":[{\"name\":\"video\",\"packaging\":\"loc\",\"role\":\"video\",\"isLive\":true,\"codec\":\"vp8\",\"bitrate\":1000000,\"framerate\":30},{\"name\":\"audio\",\"packaging\":\"loc\",\"role\":\"audio\",\"isLive\":true,\"codec\":\"opus\",\"bitrate\":64000,\"samplerate\":48000,\"channelConfig\":\"mono\"}]}",
        );
        catalog_sender.send(video_only).expect("sent");
        catalog_sender.send(with_audio).expect("sent");

        // Act
        let (source, translated) =
            first_translatable_catalog(&mut catalog_receiver, "anon/live-translate/brave-otter")
                .await
                .expect("a translatable catalog");

        // Assert: the second catalog is used, and its video is referenced by the translated one
        assert_eq!(source.tracks().len(), 2);
        let translated_tracks = translated.tracks();
        assert!(translated_tracks.iter().any(|track| track.name == "video"));
        assert_eq!(
            translated_tracks.len(),
            2 + TARGET_LANGUAGES.len() + 1 + TARGET_LANGUAGES.len()
        );
    }

    #[test]
    fn channel_count_reads_every_notation_of_the_draft() {
        // Act / Assert
        assert_eq!(channel_count(Some("mono")), 1);
        assert_eq!(channel_count(Some("stereo")), 2);
        assert_eq!(channel_count(Some("2ch")), 2);
        assert_eq!(channel_count(Some("2")), 2);
        assert_eq!(channel_count(None), 1);
    }

    #[test]
    fn every_language_and_the_source_transcript_get_a_track() {
        // Act
        let names: Vec<String> = TrackKey::all().map(TrackKey::name).collect();

        // Assert
        assert_eq!(names.len(), 2 * TARGET_LANGUAGES.len() + 1);
        assert!(names.contains(&"audio_en".to_string()));
        assert!(names.contains(&"text_zh-Hans".to_string()));
        assert!(names.contains(&"text_ja".to_string()));
    }
}

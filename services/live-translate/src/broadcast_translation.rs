use std::{collections::HashMap, future::pending, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    RequestErrorCode, Session, Subscription, TrackWriter, object::ContentExists,
    publish::PublishDoneCode, subscribe::SubscribeHandler,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    broadcast_namespace::translated_namespace,
    catalog_publication::CatalogPublication,
    language_translation::LanguageTranslation,
    source_audio::SourceAudio,
    source_catalog::{CATALOG_TRACK, SourceCatalog},
    target_language::{TARGET_LANGUAGES, TargetLanguage, find_by_track_name},
    translated_catalog::{build_translated_catalog, source_audio_track},
    translation_engine::TranslationEngine,
    translation_policy::TranslationPolicy,
};

const SUBSCRIPTION_NEVER_EXPIRES: u64 = 0;
const LANGUAGE_IDLE_GRACE: Duration = Duration::from_secs(5);
const POLICY_CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Covers the source catalog's SUBSCRIBE retries and the wait for the audio
/// track to be listed.
const FIRST_CATALOG_TIMEOUT: Duration = Duration::from_secs(30);

enum BroadcastCommand {
    Subscribe(Box<SubscribeHandler>),
    Unsubscribe(u64),
}

enum StartRefusal {
    Closed,
    Capacity,
    Failed(anyhow::Error),
}

pub(crate) struct BroadcastTranslation {
    session: Arc<Session>,
    translated_namespace: String,
    command_sender: mpsc::UnboundedSender<BroadcastCommand>,
    join_handle: JoinHandle<()>,
}

impl BroadcastTranslation {
    pub(crate) fn run(
        session: Arc<Session>,
        engine: TranslationEngine,
        capacity: Arc<Semaphore>,
        policy: Arc<TranslationPolicy>,
        broadcast_namespace: String,
    ) -> Self {
        let translated_namespace = translated_namespace(&broadcast_namespace);
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let join_handle = tokio::spawn({
            let session = session.clone();
            async move {
                if let Err(error) = serve(
                    session,
                    engine,
                    capacity,
                    policy,
                    &broadcast_namespace,
                    command_receiver,
                )
                .await
                {
                    tracing::warn!(%broadcast_namespace, ?error, "broadcast translation stopped");
                }
            }
        });
        Self {
            session,
            translated_namespace,
            command_sender,
            join_handle,
        }
    }

    pub(crate) fn subscribe(&self, handler: SubscribeHandler) {
        let _ = self
            .command_sender
            .send(BroadcastCommand::Subscribe(Box::new(handler)));
    }

    pub(crate) fn unsubscribe(&self, request_id: u64) {
        let _ = self
            .command_sender
            .send(BroadcastCommand::Unsubscribe(request_id));
    }
}

impl Drop for BroadcastTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
        let session = self.session.clone();
        let namespace = self.translated_namespace.clone();
        tokio::spawn(async move {
            let _ = session.publisher().publish_namespace_done(namespace).await;
        });
    }
}

struct LanguageSlot {
    translation: LanguageTranslation,
    idle_since: Option<Instant>,
    started_at: Instant,
    /// An always-on language: neither viewers leaving nor the time limit stop it.
    pinned: bool,
    /// The viewers' subscriptions the relay forwarded, to end with PUBLISH_DONE.
    subscriptions: HashMap<u64, Subscription>,
}

struct Serving {
    session: Arc<Session>,
    engine: TranslationEngine,
    capacity: Arc<Semaphore>,
    policy: Arc<TranslationPolicy>,
    broadcast_namespace: String,
    source_track_name: String,
    source_channels: u32,
    catalog_payload: Bytes,
    catalog_publications: HashMap<u64, CatalogPublication>,
    languages: HashMap<&'static str, LanguageSlot>,
    request_languages: HashMap<u64, &'static str>,
    source_audio: Option<SourceAudio>,
}

async fn serve(
    session: Arc<Session>,
    engine: TranslationEngine,
    capacity: Arc<Semaphore>,
    policy: Arc<TranslationPolicy>,
    broadcast_namespace: &str,
    mut command_receiver: mpsc::UnboundedReceiver<BroadcastCommand>,
) -> Result<()> {
    let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel();
    let _source_catalog = SourceCatalog::run(
        session.clone(),
        broadcast_namespace.to_string(),
        catalog_sender,
    );
    let (source_catalog, translated_catalog) = tokio::time::timeout(
        FIRST_CATALOG_TIMEOUT,
        first_translatable_catalog(
            &mut catalog_receiver,
            broadcast_namespace,
            &TARGET_LANGUAGES,
        ),
    )
    .await
    .context("source catalog did not list Opus audio in time")??;
    let source_track = source_audio_track(&source_catalog)
        .context("source catalog has no Opus audio to translate")?;
    let mut serving = Serving {
        session: session.clone(),
        engine,
        capacity,
        policy,
        broadcast_namespace: broadcast_namespace.to_string(),
        source_track_name: source_track.name.clone(),
        source_channels: channel_count(source_track.channel_config.as_deref()),
        catalog_payload: Bytes::from(serde_json::to_vec(&translated_catalog)?),
        catalog_publications: HashMap::new(),
        languages: HashMap::new(),
        request_languages: HashMap::new(),
        source_audio: None,
    };
    let namespace = translated_namespace(broadcast_namespace);
    session
        .publisher()
        .publish_namespace(namespace.clone())
        .await
        .context("announce the translated namespace")?;
    tracing::info!(%namespace, source_track = %serving.source_track_name, "translation ready");
    serving.start_always_on();
    let mut policy_check = tokio::time::interval(POLICY_CHECK_INTERVAL);
    loop {
        let idle_deadline = serving.earliest_idle_deadline();
        tokio::select! {
            _ = policy_check.tick() => {
                serving.enforce_limits().await;
                serving.start_always_on();
            }
            command = command_receiver.recv() => match command {
                Some(BroadcastCommand::Subscribe(handler)) => serving.subscribe(*handler).await,
                Some(BroadcastCommand::Unsubscribe(request_id)) => serving.unsubscribe(request_id),
                None => return Ok(()),
            },
            catalog = catalog_receiver.recv() => if let Some(catalog) = catalog {
                serving.update_catalog(&catalog)?;
            },
            _ = sleep_until(idle_deadline) => serving.stop_idle_languages(),
        }
    }
}

impl Serving {
    async fn subscribe(&mut self, handler: SubscribeHandler) {
        let track_name = handler.track_name.clone();
        let result = if track_name == CATALOG_TRACK {
            self.subscribe_catalog(&handler).await
        } else if let Some(language) = find_by_track_name(&track_name) {
            self.subscribe_language(&handler, language).await
        } else {
            reject(
                &handler,
                RequestErrorCode::TrackDoesNotExist,
                "unknown track",
            )
            .await;
            Ok(())
        };
        if let Err(error) = result {
            tracing::warn!(%track_name, ?error, "SUBSCRIBE failed");
        }
    }

    async fn subscribe_catalog(&mut self, handler: &SubscribeHandler) -> Result<()> {
        let (_, writer) = self.accept(handler).await?;
        self.catalog_publications.insert(
            handler.request_id(),
            CatalogPublication::run(writer, self.catalog_payload.clone()),
        );
        Ok(())
    }

    async fn subscribe_language(
        &mut self,
        handler: &SubscribeHandler,
        language: &'static TargetLanguage,
    ) -> Result<()> {
        if let Err(refusal) = self.start_language(language, false) {
            let reason = match &refusal {
                StartRefusal::Closed => {
                    format!(
                        "closed outside the open hours {}",
                        self.policy
                            .open_hours
                            .as_ref()
                            .expect("closed only with open hours")
                    )
                }
                StartRefusal::Capacity => "translation capacity reached".to_string(),
                StartRefusal::Failed(_) => "translation failed to start".to_string(),
            };
            reject(handler, RequestErrorCode::InternalError, &reason).await;
            return match refusal {
                StartRefusal::Failed(error) => Err(error),
                _ => Ok(()),
            };
        }
        let (subscription, writer) = self.accept(handler).await?;
        if let Some(slot) = self.languages.get_mut(language.code) {
            slot.translation.replace_writer(Some(writer));
            slot.idle_since = None;
            slot.subscriptions
                .insert(handler.request_id(), subscription);
        }
        self.request_languages
            .insert(handler.request_id(), language.code);
        Ok(())
    }

    /// Starts the language's Gemini session unless it runs already. A pinned
    /// start is an always-on language's; a later subscriber keeps it pinned.
    fn start_language(
        &mut self,
        language: &'static TargetLanguage,
        pinned: bool,
    ) -> Result<(), StartRefusal> {
        if !self.policy.is_open() {
            return Err(StartRefusal::Closed);
        }
        if self.languages.contains_key(language.code) {
            return Ok(());
        }
        let Ok(capacity_permit) = self.capacity.clone().try_acquire_owned() else {
            return Err(StartRefusal::Capacity);
        };
        let source_audio = self.source_audio.get_or_insert_with(|| {
            SourceAudio::run(
                self.session.clone(),
                self.broadcast_namespace.clone(),
                self.source_track_name.clone(),
                self.source_channels,
            )
        });
        let translation = LanguageTranslation::run(
            &self.engine,
            language,
            source_audio.subscribe(),
            capacity_permit,
        )
        .map_err(StartRefusal::Failed)?;
        self.languages.insert(
            language.code,
            LanguageSlot {
                translation,
                idle_since: None,
                started_at: Instant::now(),
                pinned,
                subscriptions: HashMap::new(),
            },
        );
        Ok(())
    }

    /// Also called at every policy check, so a start refused while closed, or
    /// failed, is retried.
    fn start_always_on(&mut self) {
        for language in self.policy.always_on.clone() {
            if self.languages.contains_key(language.code) {
                continue;
            }
            match self.start_language(language, true) {
                Ok(()) => {
                    tracing::info!(language = language.code, "translation started, always on")
                }
                Err(StartRefusal::Closed) => {}
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
    /// codec is known, reaches every catalog subscriber at once. The audio
    /// track being translated stays the one of the first catalog.
    fn update_catalog(&mut self, source: &msf::Catalog) -> Result<()> {
        let Some(translated) =
            build_translated_catalog(source, &self.broadcast_namespace, &TARGET_LANGUAGES)
        else {
            return Ok(());
        };
        let payload = Bytes::from(serde_json::to_vec(&translated)?);
        if payload == self.catalog_payload {
            return Ok(());
        }
        self.catalog_payload = payload;
        for publication in self.catalog_publications.values() {
            publication.update(self.catalog_payload.clone());
        }
        tracing::info!(
            namespace = %translated_namespace(&self.broadcast_namespace),
            subscribers = self.catalog_publications.len(),
            "translated catalog updated"
        );
        Ok(())
    }

    fn unsubscribe(&mut self, request_id: u64) {
        if self.catalog_publications.remove(&request_id).is_some() {
            return;
        }
        let Some(code) = self.request_languages.remove(&request_id) else {
            return;
        };
        if let Some(slot) = self.languages.get_mut(code) {
            slot.translation.replace_writer(None);
            slot.idle_since = Some(Instant::now());
            slot.subscriptions.remove(&request_id);
        }
    }

    /// Viewers of an ended language get PUBLISH_DONE and may subscribe again.
    async fn enforce_limits(&mut self) {
        let now = Instant::now();
        let closed = !self.policy.is_open();
        let ended: Vec<(&'static str, &'static str)> = self
            .languages
            .iter()
            .filter_map(|(code, slot)| {
                if !slot.pinned && now >= slot.started_at + self.policy.language_time_limit {
                    Some((*code, "time limit"))
                } else if closed {
                    Some((*code, "closed"))
                } else {
                    None
                }
            })
            .collect();
        for (code, reason) in ended {
            let Some(slot) = self.languages.remove(code) else {
                continue;
            };
            for (request_id, subscription) in slot.subscriptions {
                self.request_languages.remove(&request_id);
                if let Err(error) = self
                    .session
                    .publisher()
                    .publish_done(
                        &subscription,
                        PublishDoneCode::TrackEnded,
                        0,
                        reason.to_string(),
                    )
                    .await
                {
                    tracing::warn!(request_id, ?error, "send PUBLISH_DONE");
                }
            }
            tracing::info!(
                language = code,
                reason,
                ran_minutes = (now - slot.started_at).as_secs() / 60,
                "translation stopped"
            );
        }
        if self.languages.is_empty() {
            self.source_audio = None;
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

    fn stop_idle_languages(&mut self) {
        let now = Instant::now();
        self.languages.retain(|code, slot| {
            let expired = !slot.pinned
                && slot
                    .idle_since
                    .is_some_and(|idle_since| idle_since + LANGUAGE_IDLE_GRACE <= now);
            if expired {
                tracing::info!(language = code, "translation stopped, no viewers");
            }
            !expired
        });
        if self.languages.is_empty() {
            self.source_audio = None;
        }
    }

    async fn accept(&self, handler: &SubscribeHandler) -> Result<(Subscription, TrackWriter)> {
        let track_alias = handler
            .ok(SUBSCRIPTION_NEVER_EXPIRES, ContentExists::False)
            .await
            .context("send SUBSCRIBE_OK")?;
        let subscription = handler.into_subscription(track_alias);
        let writer = TrackWriter::new(self.session.publisher().create_stream(&subscription), 0);
        Ok((subscription, writer))
    }
}

async fn reject(handler: &SubscribeHandler, error_code: RequestErrorCode, reason: &str) {
    if let Err(error) = handler.error(error_code, reason.to_string()).await {
        tracing::warn!(?error, "send SUBSCRIBE_ERROR");
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
    catalog_receiver: &mut mpsc::UnboundedReceiver<msf::Catalog>,
    broadcast_namespace: &str,
    languages: &[TargetLanguage],
) -> Result<(msf::Catalog, msf::Catalog)> {
    while let Some(source) = catalog_receiver.recv().await {
        if let Some(translated) = build_translated_catalog(&source, broadcast_namespace, languages)
        {
            return Ok((source, translated));
        }
        tracing::debug!(%broadcast_namespace, "source catalog lists no Opus audio yet");
    }
    anyhow::bail!("source catalog ended before listing Opus audio")
}

fn channel_count(channel_config: Option<&str>) -> u32 {
    match channel_config {
        Some("mono") | None => 1,
        Some(other) => other.trim_end_matches("ch").parse().unwrap_or(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(json: &str) -> msf::Catalog {
        serde_json::from_str(json).expect("catalog")
    }

    #[tokio::test]
    async fn the_first_catalog_with_opus_audio_starts_the_translation() {
        // Arrange: the publisher lists the video first and the audio with its next catalog
        let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel();
        let video_only = catalog(
            "{\"version\":1,\"tracks\":[{\"name\":\"video\",\"packaging\":\"loc\",\"role\":\"video\",\"isLive\":true,\"codec\":\"vp8\"}]}",
        );
        let with_audio = catalog(
            "{\"version\":1,\"tracks\":[{\"name\":\"video\",\"packaging\":\"loc\",\"role\":\"video\",\"isLive\":true,\"codec\":\"vp8\"},{\"name\":\"audio\",\"packaging\":\"loc\",\"role\":\"audio\",\"isLive\":true,\"codec\":\"opus\",\"samplerate\":48000,\"channelConfig\":\"mono\"}]}",
        );
        catalog_sender.send(video_only).expect("sent");
        catalog_sender.send(with_audio).expect("sent");

        // Act
        let (source, translated) = first_translatable_catalog(
            &mut catalog_receiver,
            "anon/live-translate/brave-otter",
            &TARGET_LANGUAGES,
        )
        .await
        .expect("a translatable catalog");

        // Assert: the second catalog is used, and its video is referenced by the translated one
        assert_eq!(source.tracks.as_ref().map(Vec::len), Some(2));
        let translated_tracks = translated.tracks.expect("tracks");
        assert!(translated_tracks.iter().any(|track| track.name == "video"));
        assert_eq!(translated_tracks.len(), 2 + TARGET_LANGUAGES.len());
    }

    #[test]
    fn channel_count_reads_the_browser_publisher_notation() {
        // Act / Assert
        assert_eq!(channel_count(Some("mono")), 1);
        assert_eq!(channel_count(Some("2ch")), 2);
        assert_eq!(channel_count(Some("2")), 2);
        assert_eq!(channel_count(None), 1);
    }
}

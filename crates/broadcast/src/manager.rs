use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose};
use bytes::Bytes;
use mediapack::{
    aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord, mp4::Fmp4TrackMuxer,
};
use moqt::{
    ClientConfig, DUAL, Endpoint, Session, SessionEvent, TerminationErrorCode, TransportStats,
    publish::{Origin, OutgoingObject, Retention, WriteOutcome},
    subscribe::SubscribeHandler,
};
use msf::{
    CATALOG_TRACK_NAME, Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};
use tokio::{sync::Mutex, task::JoinHandle, time::Instant};

use crate::stream_ledger::{StreamLedger, StreamRecord};

pub const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
pub(crate) const TIMELINE_TRACK_NAME: &str = "timeline";
const CMAF_TRACK_SUFFIX: &str = "_cmaf";
const CHAT_TRACK_NAME: &str = "chat";
const CHAT_EVENT_TYPE: &str = "com.skyway.chat.v1";
/// SUBSCRIBE_ERROR code, draft-ietf-moq-transport-14 §13.1.2.
const SUBSCRIBE_TRACK_DOES_NOT_EXIST: u64 = 0x4;
/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for an evicted or never-cached range can still be answered here.
const FETCH_CACHE_RETENTION: Duration = Duration::from_secs(60);
/// Half the relay's default RELAY_CACHE_TTL_SECS: draft-ietf-moq-msf-01 §5
/// republishes the catalog before it can fall out of a relay cache, so a
/// viewer arriving long after the last change still finds one.
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct MoqtTarget {
    pub url: String,
    pub auth_token: Option<String>,
    pub verify_certificate: bool,
}

#[derive(Clone)]
pub struct MoqtManager {
    target: Option<MoqtTarget>,
    inner: Arc<Mutex<ManagerState>>,
    ledger: StreamLedger,
}

#[derive(Default)]
struct ManagerState {
    backend: Option<Arc<ConnectedPublisher>>,
}

type TrackKey = (String, String);

#[derive(Default)]
struct BackendState {
    announced_namespaces: HashSet<String>,
    catalogs: HashMap<String, CatalogMetadata>,
    catalog_sent_at: HashMap<String, Instant>,
    disconnected: bool,
}

impl BackendState {
    fn catalog_refresh_due(&self, namespace_path: &str) -> bool {
        self.catalog_sent_at
            .get(namespace_path)
            .is_some_and(|sent_at| sent_at.elapsed() >= CATALOG_REFRESH_INTERVAL)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub label: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// The CMAF init segment of the track, Base64 for the catalog `initData`
    /// (draft-ietf-moq-cmsf-01 §3.1).
    pub init_segment: String,
}

impl VideoTrackInfo {
    pub fn from_record(record: &AvcDecoderConfigurationRecord, label: String) -> Result<Self> {
        let sps = record.sequence_parameter_set()?;
        let init_segment = Fmp4TrackMuxer::video(record.clone()).init_segment()?;
        Ok(Self {
            label,
            codec: record.codec_string(),
            width: sps.width,
            height: sps.height,
            init_segment: general_purpose::STANDARD.encode(init_segment),
        })
    }
}

pub fn cmaf_track_name(track_name: &str) -> String {
    format!("{track_name}{CMAF_TRACK_SUFFIX}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CatalogMetadata {
    video_tracks: BTreeMap<String, VideoTrackInfo>,
    audio_config: Option<AudioSpecificConfig>,
}

struct ConnectedPublisher {
    endpoint: Endpoint<DUAL>,
    session: Arc<Session>,
    origin: Origin,
    state: Arc<Mutex<BackendState>>,
    ledger: StreamLedger,
    event_task: JoinHandle<()>,
}

impl MoqtManager {
    pub fn new(target: Option<MoqtTarget>) -> Self {
        Self {
            target,
            inner: Arc::new(Mutex::new(ManagerState::default())),
            ledger: StreamLedger::default(),
        }
    }

    pub fn streams(&self) -> Vec<StreamRecord> {
        self.ledger.snapshot()
    }

    pub async fn setup_namespace(&self, namespace: &[String]) -> Result<()> {
        let Some(target) = &self.target else {
            return Ok(());
        };

        self.ensure_backend(target)
            .await?
            .setup_namespace(namespace)
            .await
    }

    /// Returns whether the object was sent to the relay: the object is
    /// numbered and cached either way, but the track may not be published yet,
    /// or the writer may still wait for the next group start.
    pub async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<bool> {
        let Some(target) = &self.target else {
            return Ok(false);
        };

        self.ensure_backend(target)
            .await?
            .send_object(namespace, track_name, object)
            .await
    }

    pub async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        let Some(target) = &self.target else {
            return Ok(());
        };

        self.ensure_backend(target)
            .await?
            .update_video_catalog(namespace, track_name, info)
            .await
    }

    pub async fn update_audio_catalog(
        &self,
        namespace: &[String],
        config: AudioSpecificConfig,
    ) -> Result<()> {
        let Some(target) = &self.target else {
            return Ok(());
        };

        self.ensure_backend(target)
            .await?
            .update_audio_catalog(namespace, config)
            .await
    }

    /// `None` until the relay session exists; the lock is also held for the
    /// whole connect, so a caller during it sees `None` as well.
    /// Returns once the CONNECTION_CLOSE has been sent; exiting before that
    /// leaves the relay to end the session only after its idle timeout.
    pub async fn close(&self) {
        let backend = self.inner.lock().await.backend.clone();
        if let Some(backend) = backend {
            backend.close().await;
        }
    }

    pub fn transport_stats(&self) -> Option<TransportStats> {
        let backend = self.inner.try_lock().ok()?.backend.clone()?;
        Some(backend.transport_stats())
    }

    async fn ensure_backend(&self, target: &MoqtTarget) -> Result<Arc<ConnectedPublisher>> {
        let mut guard = self.inner.lock().await;
        if guard.backend.is_none() {
            guard.backend = Some(Arc::new(
                ConnectedPublisher::connect(target, self.ledger.clone()).await?,
            ));
        }
        let backend = guard
            .backend
            .as_ref()
            .ok_or_else(|| anyhow!("publisher backend not initialized"))?
            .clone();
        drop(guard);
        Ok(backend)
    }
}

impl ConnectedPublisher {
    async fn connect(target: &MoqtTarget, ledger: StreamLedger) -> Result<Self> {
        let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: target.verify_certificate,
            authorization_token: target.auth_token.clone(),
        })?;
        let connecting = endpoint
            .connect(&target.url)
            .await
            .context("connect moqt transport")?;
        let session = Arc::new(connecting.await.context("establish moqt session")?);

        let origin = Origin::new(session.clone());
        let state = Arc::new(Mutex::new(BackendState::default()));
        let event_task = Self::spawn_event_loop(
            session.clone(),
            origin.clone(),
            state.clone(),
            ledger.clone(),
        );

        Ok(Self {
            endpoint,
            session,
            origin,
            state,
            ledger,
            event_task,
        })
    }

    async fn close(&self) {
        self.session
            .close_with_error(TerminationErrorCode::NoError, "");
        self.endpoint.wait_idle().await;
    }

    fn transport_stats(&self) -> TransportStats {
        self.session.transport_stats()
    }

    fn spawn_event_loop(
        session: Arc<Session>,
        origin: Origin,
        state: Arc<Mutex<BackendState>>,
        ledger: StreamLedger,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let event = match session.receive_event().await {
                    Ok(event) => event,
                    Err(err) => {
                        tracing::warn!(?err, "MoQT event loop ended");
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                };

                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(namespace = %handler.track_namespace, ?err, "failed to ack PUBLISH_NAMESPACE");
                        }
                    }
                    SessionEvent::PublishNamespaceDone(handler) => {
                        tracing::info!(namespace = %handler.track_namespace, "PUBLISH_NAMESPACE_DONE received");
                    }
                    SessionEvent::SubscribeNameSpace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(prefix = %handler.track_namespace_prefix, ?err, "failed to ack SUBSCRIBE_NAMESPACE");
                        }
                    }
                    SessionEvent::Subscribe(handler) => {
                        Self::serve_subscribe(&origin, &state, handler).await;
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        let request_id = handler.subscribe_id();
                        match origin.release(request_id).await {
                            Some((namespace, track_name)) => {
                                ledger.finish(&track_name);
                                tracing::info!(request_id, %namespace, %track_name, "UNSUBSCRIBE received; track released");
                            }
                            None => {
                                tracing::warn!(request_id, "UNSUBSCRIBE for unknown request");
                            }
                        }
                    }
                    SessionEvent::UnsubscribeNamespace(handler) => {
                        tracing::info!(prefix = %handler.track_namespace_prefix(), "UNSUBSCRIBE_NAMESPACE received");
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => {
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                    SessionEvent::Publish(handler) => {
                        tracing::warn!(namespace = %handler.track_namespace, track = %handler.track_name, "unexpected PUBLISH received");
                    }
                    SessionEvent::Fetch(handler) => {
                        let request_id = handler.request_id;
                        if let Err(err) = origin.serve_fetch(handler).await {
                            tracing::warn!(request_id, ?err, "failed to answer FETCH");
                        }
                    }
                    SessionEvent::FetchCancel(handler) => {
                        origin.cancel_fetch(handler.request_id()).await;
                    }
                    SessionEvent::TrackStatus(handler) => {
                        let request_id = handler.request_id();
                        if let Err(err) = origin.answer_track_status(handler).await {
                            tracing::warn!(request_id, ?err, "failed to answer TRACK_STATUS");
                        }
                    }
                    event @ (SessionEvent::GoAway(_)
                    | SessionEvent::MaxRequestId(_)
                    | SessionEvent::RequestsBlocked(_)
                    | SessionEvent::PublishNamespaceCancel(_)
                    | SessionEvent::PublishDone(_)
                    | SessionEvent::SubscribeUpdate(_)) => {
                        tracing::debug!(?event, "unhandled control message");
                    }
                }
            }
        })
    }

    async fn setup_namespace(&self, namespace: &[String]) -> Result<()> {
        let namespace_path = namespace.join("/");
        let announce_needed = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            guard.catalogs.entry(namespace_path.clone()).or_default();
            !guard.announced_namespaces.contains(&namespace_path)
        };

        if announce_needed {
            self.session
                .publisher()
                .publish_namespace(namespace_path.clone())
                .await
                .with_context(|| format!("publish_namespace {}", namespace_path))?;
            let mut guard = self.state.lock().await;
            guard.announced_namespaces.insert(namespace_path.clone());
            tracing::info!(namespace = %namespace_path, "namespace published");
        }
        Ok(())
    }

    async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<bool> {
        let key = (namespace.join("/"), track_name.to_string());
        if self.state.lock().await.catalog_refresh_due(&key.0) {
            self.send_catalog(&key.0).await?;
        }
        if self.state.lock().await.disconnected {
            bail!("MoQ publisher disconnected");
        }
        ensure_origin_track(&self.origin, &key).await;
        let bytes = object.payload.len();
        match self.origin.write(&key.0, &key.1, object).await? {
            WriteOutcome::Sent(placement) => {
                if placement.starts_group {
                    self.ledger.open(&key.1, placement.location.group_id);
                }
                self.ledger.add_object(&key.1, bytes);
                Ok(true)
            }
            WriteOutcome::Stopped(placement) => {
                if placement.starts_group {
                    self.ledger.open(&key.1, placement.location.group_id);
                }
                self.ledger.finish(&key.1);
                Ok(false)
            }
            WriteOutcome::NotSent => Ok(false),
        }
    }

    async fn serve_subscribe(
        origin: &Origin,
        state: &Mutex<BackendState>,
        handler: SubscribeHandler,
    ) {
        let namespace = handler.track_namespace.clone();
        let track_name = handler.track_name.clone();
        let subscribable = {
            let guard = state.lock().await;
            guard
                .catalogs
                .get(&namespace)
                .and_then(|metadata| build_catalog(&namespace, metadata).ok())
                .is_some_and(|catalog| awaits_subscribe(&catalog, &track_name))
        };
        if !subscribable {
            if let Err(err) = handler
                .error(
                    SUBSCRIBE_TRACK_DOES_NOT_EXIST,
                    format!("{track_name} is not a CMAF track of the catalog"),
                )
                .await
            {
                tracing::warn!(%namespace, %track_name, ?err, "failed to reject SUBSCRIBE");
            }
            return;
        }
        ensure_origin_track(origin, &(namespace.clone(), track_name.clone())).await;
        if let Err(err) = origin.accept_subscribe(handler).await {
            tracing::warn!(%namespace, %track_name, ?err, "failed to accept SUBSCRIBE");
        }
    }

    async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        let namespace_path = namespace.join("/");

        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            if metadata.video_tracks.get(track_name) == Some(&info) {
                return Ok(());
            }
            metadata.video_tracks.insert(track_name.to_string(), info);
        }
        self.send_catalog(&namespace_path).await
    }

    async fn update_audio_catalog(
        &self,
        namespace: &[String],
        config: AudioSpecificConfig,
    ) -> Result<()> {
        let namespace_path = namespace.join("/");

        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            if metadata.audio_config.as_ref() == Some(&config) {
                return Ok(());
            }
            metadata.audio_config = Some(config);
        }
        self.send_catalog(&namespace_path).await
    }

    /// Every track the catalog lists but CMAF is published before the catalog
    /// is sent, so a viewer subscribing to one it read there finds the relay
    /// already ingesting it instead of forwarding the SUBSCRIBE here. CMAF
    /// tracks wait for that SUBSCRIBE, so the uplink carries them only while
    /// someone watches them.
    async fn send_catalog(&self, namespace_path: &str) -> Result<()> {
        let catalog = {
            let guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard
                .catalogs
                .get(namespace_path)
                .cloned()
                .unwrap_or_default();
            build_catalog(namespace_path, &metadata)?
        };
        let published_tracks = catalog
            .tracks
            .iter()
            .flatten()
            .filter(|track| !is_cmaf(track))
            .map(|track| track.name.as_str());
        for track_name in published_tracks.chain([CATALOG_TRACK_NAME]) {
            self.publish_track((namespace_path.to_string(), track_name.to_string()))
                .await?;
        }
        let payload = Bytes::from(serde_json::to_vec(&catalog).context("serialize msf catalog")?);
        tracing::debug!(namespace = %namespace_path, catalog = %String::from_utf8_lossy(&payload), "catalog snapshot");
        self.state
            .lock()
            .await
            .catalog_sent_at
            .insert(namespace_path.to_string(), Instant::now());
        let bytes = payload.len();
        match self
            .origin
            .write_group(namespace_path, CATALOG_TRACK_NAME, payload)
            .await
            .context("send catalog group")?
        {
            WriteOutcome::Sent(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.add_object(CATALOG_TRACK_NAME, bytes);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(namespace = %namespace_path, group_id = placement.location.group_id, "catalog sent");
            }
            WriteOutcome::Stopped(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(namespace = %namespace_path, "subscriber stopped the catalog group");
            }
            WriteOutcome::NotSent => {}
        }
        Ok(())
    }

    async fn publish_track(&self, key: TrackKey) -> Result<()> {
        ensure_origin_track(&self.origin, &key).await;
        self.origin.publish(&key.0, &key.1).await
    }
}

impl Drop for ConnectedPublisher {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

/// The catalog is kept for as long as the publisher runs, because a viewer
/// that joins a subscription the relay already holds can only obtain it by
/// FETCH, however long ago it was published.
async fn ensure_origin_track(origin: &Origin, key: &TrackKey) {
    let retention = if key.1 == CATALOG_TRACK_NAME {
        Retention::NewestOnly
    } else {
        Retention::For(FETCH_CACHE_RETENTION)
    };
    origin
        .ensure_track(&key.0, &key.1, now_unix().as_micros() as u64, retention)
        .await;
}

fn is_cmaf(track: &Track) -> bool {
    track.packaging == Packaging::Known(KnownPackaging::Cmaf)
}

fn awaits_subscribe(catalog: &Catalog, track_name: &str) -> bool {
    catalog
        .tracks
        .iter()
        .flatten()
        .any(|track| track.name == track_name && is_cmaf(track))
}

fn build_catalog(namespace_path: &str, metadata: &CatalogMetadata) -> Result<Catalog> {
    let namespace = Some(namespace_path.to_string());
    let video_role = TrackRole::Known(KnownTrackRole::Video);
    let alt_group = (metadata.video_tracks.len() > 1).then_some(1);
    let cmaf_alt_group = alt_group.map(|group| group + 1);

    let mut tracks = Vec::new();
    for (name, info) in &metadata.video_tracks {
        let mut loc = track(&namespace, name, KnownPackaging::Loc, video_role.clone());
        loc.label = Some(info.label.clone());
        loc.alt_group = alt_group;
        loc.codec = Some(info.codec.clone());
        loc.mime_type = Some("video/h264".to_string());
        loc.framerate = Some(30.0);
        loc.width = Some(info.width);
        loc.height = Some(info.height);
        let cmaf = cmaf_sibling(&loc, "video/mp4", info.init_segment.clone(), cmaf_alt_group);
        tracks.push(loc);
        tracks.push(cmaf);
    }

    let audio_role = TrackRole::Known(KnownTrackRole::Audio);
    let mut audio = track(
        &namespace,
        AUDIO_TRACK_NAME,
        KnownPackaging::Loc,
        audio_role,
    );
    audio.label = Some("Audio".to_string());
    audio.codec = Some("mp4a.40.2".to_string());
    audio.mime_type = Some("audio/aac".to_string());
    if let Some(config) = &metadata.audio_config {
        audio.sample_rate = Some(config.sample_rate);
        audio.channel_config = Some(channel_config_label(config.channel_count()));
        audio.init_data = Some(general_purpose::STANDARD.encode(config.to_bytes()));
    }
    let audio_cmaf = match &metadata.audio_config {
        Some(config) => Some(cmaf_sibling(
            &audio,
            "audio/mp4",
            general_purpose::STANDARD.encode(Fmp4TrackMuxer::audio(config.clone()).init_segment()?),
            None,
        )),
        None => None,
    };
    tracks.push(audio);
    tracks.extend(audio_cmaf);

    let mut timeline = track(
        &namespace,
        TIMELINE_TRACK_NAME,
        KnownPackaging::MediaTimeline,
        TrackRole::Known(KnownTrackRole::MediaTimeline),
    );
    timeline.label = Some("Media timeline".to_string());
    timeline.depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        cmaf_track_name(VIDEO_TRACK_NAME),
    ]);
    timeline.mime_type = Some("application/json".to_string());
    tracks.push(timeline);

    let mut chat = track(
        &namespace,
        CHAT_TRACK_NAME,
        KnownPackaging::EventTimeline,
        TrackRole::Other(CHAT_TRACK_NAME.to_string()),
    );
    chat.event_type = Some(CHAT_EVENT_TYPE.to_string());
    chat.label = Some("Chat".to_string());
    chat.depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        AUDIO_TRACK_NAME.to_string(),
    ]);
    chat.mime_type = Some("application/json".to_string());
    tracks.push(chat);

    Ok(Catalog::new(tracks, now_unix().as_millis() as u64))
}

fn cmaf_sibling(loc: &Track, mime_type: &str, init_data: String, alt_group: Option<u64>) -> Track {
    let mut cmaf = loc.cmaf_sibling(&cmaf_track_name(&loc.name), mime_type, init_data, alt_group);
    cmaf.label = loc.label.as_ref().map(|label| format!("{label} (CMAF)"));
    cmaf
}

fn track(
    namespace: &Option<String>,
    name: &str,
    packaging: KnownPackaging,
    role: TrackRole,
) -> Track {
    let mut track = Track::new(name, packaging, true);
    track.namespace = namespace.clone();
    track.role = Some(role);
    track
}

fn channel_config_label(channels: u8) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

pub(crate) fn now_unix() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_SPS: [u8; 24] = [
        0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04,
        0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const FIXTURE_PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];

    fn metadata_with_video_and_audio() -> CatalogMetadata {
        let record = AvcDecoderConfigurationRecord::from_parameter_sets(
            vec![Bytes::from_static(&FIXTURE_SPS)],
            vec![Bytes::from_static(&FIXTURE_PPS)],
        )
        .unwrap();
        let mut video_tracks = BTreeMap::new();
        video_tracks.insert(
            VIDEO_TRACK_NAME.to_string(),
            VideoTrackInfo::from_record(&record, "Video".to_string()).unwrap(),
        );
        CatalogMetadata {
            video_tracks,
            audio_config: Some(AudioSpecificConfig::new(2, 48_000, 1)),
        }
    }

    fn track_named<'a>(catalog: &'a Catalog, name: &str) -> &'a Track {
        catalog
            .tracks
            .as_ref()
            .unwrap()
            .iter()
            .find(|track| track.name == name)
            .unwrap_or_else(|| panic!("missing track {name}"))
    }

    #[test]
    fn transport_stats_is_none_before_the_relay_session_exists() {
        // Arrange
        let manager = MoqtManager::new(None);

        // Act / Assert
        assert_eq!(manager.transport_stats(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn catalog_is_due_again_once_the_refresh_interval_has_passed() {
        // Arrange
        let mut state = BackendState::default();
        state
            .catalog_sent_at
            .insert("live/test".to_string(), Instant::now());

        // Act / Assert
        tokio::time::advance(CATALOG_REFRESH_INTERVAL - Duration::from_secs(1)).await;
        assert!(!state.catalog_refresh_due("live/test"));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(state.catalog_refresh_due("live/test"));
        assert!(!state.catalog_refresh_due("live/other"));
    }

    #[test]
    fn only_the_cmaf_tracks_of_the_catalog_await_subscribe() {
        // Arrange
        let catalog = build_catalog("live/test", &metadata_with_video_and_audio()).unwrap();

        // Act / Assert
        assert!(awaits_subscribe(&catalog, "video_cmaf"));
        assert!(awaits_subscribe(&catalog, "audio_cmaf"));
        assert!(!awaits_subscribe(&catalog, "video"));
        assert!(!awaits_subscribe(&catalog, "timeline"));
        assert!(!awaits_subscribe(&catalog, "video_720p_cmaf"));
    }

    #[test]
    fn catalog_pairs_each_media_track_with_a_cmaf_track_carrying_its_init_segment() {
        // Arrange
        let metadata = metadata_with_video_and_audio();

        // Act
        let catalog = build_catalog("live/test", &metadata).unwrap();

        // Assert
        let video = track_named(&catalog, "video");
        assert_eq!(video.packaging, Packaging::Known(KnownPackaging::Loc));
        assert_eq!(video.init_data, None);
        let video_cmaf = track_named(&catalog, "video_cmaf");
        assert_eq!(video_cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(video_cmaf.mime_type.as_deref(), Some("video/mp4"));
        assert_eq!(video_cmaf.codec, video.codec);
        assert_eq!(video_cmaf.max_grp_sap_starting_type, Some(1));
        assert!(
            video_cmaf
                .init_data
                .as_ref()
                .is_some_and(|data| !data.is_empty())
        );
        let audio_cmaf = track_named(&catalog, "audio_cmaf");
        assert_eq!(audio_cmaf.packaging, Packaging::Known(KnownPackaging::Cmaf));
        assert_eq!(audio_cmaf.mime_type.as_deref(), Some("audio/mp4"));
        assert!(
            audio_cmaf
                .init_data
                .as_ref()
                .is_some_and(|data| !data.is_empty())
        );
        assert_ne!(
            audio_cmaf.init_data,
            track_named(&catalog, "audio").init_data
        );
    }
}

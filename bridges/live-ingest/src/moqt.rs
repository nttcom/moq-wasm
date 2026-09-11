use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use media_streaming_format::{
    Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};
use mediapack::h264::AvcDecoderConfigurationRecord;
use moqt::{
    ClientConfig, ContentExists, Endpoint, QUIC, Session, SessionEvent, TrackWriter,
    TransportProtocol, TransportSendError, WEBTRANSPORT,
};
use tokio::sync::Mutex;

pub(crate) const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
const CATALOG_TRACK_NAME: &str = "catalog";
const CHAT_TRACK_NAME: &str = "chat";
const CHAT_EVENT_TYPE: &str = "com.skyway.chat.v1";
/// FETCH_ERROR code NOT_SUPPORTED, draft-ietf-moq-transport-14 §13.1.5.
const FETCH_NOT_SUPPORTED: u64 = 0x3;

#[derive(Clone)]
pub struct MoqtManager {
    url: Option<String>,
    inner: Arc<Mutex<ManagerState>>,
}

#[derive(Default)]
struct ManagerState {
    backend: Option<Arc<PublisherBackend>>,
}

struct BackendState<T: TransportProtocol> {
    announced_namespaces: HashSet<String>,
    tracks: HashMap<(String, String), Option<TrackWriter<T>>>,
    subscribed_tracks: HashMap<u64, (String, String)>,
    catalogs: HashMap<String, CatalogMetadata>,
    disconnected: bool,
}

impl<T: TransportProtocol> Default for BackendState<T> {
    fn default() -> Self {
        Self {
            announced_namespaces: HashSet::new(),
            tracks: HashMap::new(),
            subscribed_tracks: HashMap::new(),
            catalogs: HashMap::new(),
            disconnected: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub label: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
}

impl VideoTrackInfo {
    pub fn from_record(record: &AvcDecoderConfigurationRecord, label: String) -> Result<Self> {
        let sps = record.sequence_parameter_set()?;
        Ok(Self {
            label,
            codec: record.codec_string(),
            width: sps.width,
            height: sps.height,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CatalogMetadata {
    video_tracks: BTreeMap<String, VideoTrackInfo>,
    audio_sample_rate: Option<u32>,
    audio_channels: Option<u8>,
}

struct ConnectedPublisher<T: TransportProtocol> {
    session: Arc<Session<T>>,
    state: Arc<Mutex<BackendState<T>>>,
    event_task: tokio::task::JoinHandle<()>,
}

enum PublisherBackend {
    Quic(ConnectedPublisher<QUIC>),
    WebTransport(ConnectedPublisher<WEBTRANSPORT>),
}

impl MoqtManager {
    pub fn new(url: Option<String>) -> Self {
        Self {
            url,
            inner: Arc::new(Mutex::new(ManagerState::default())),
        }
    }

    /// Announce namespace (once) and prepare tracks (video/audio) by waiting for SubscribeOk.
    pub async fn setup_namespace(&self, namespace: &[String]) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()), // MoQ 出力なし
        };

        self.ensure_backend(&url)
            .await?
            .setup_namespace(namespace)
            .await
    }

    pub async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        rotate_group: bool,
        payload: Vec<u8>,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
            .await?
            .send_object(namespace, track_name, rotate_group, payload)
            .await
    }

    pub async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
            .await?
            .update_video_catalog(namespace, track_name, info)
            .await
    }

    pub async fn update_audio_catalog(
        &self,
        namespace: &[String],
        sample_rate: u32,
        channels: u8,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
            .await?
            .update_audio_catalog(namespace, sample_rate, channels)
            .await
    }

    async fn ensure_backend(&self, url: &str) -> Result<Arc<PublisherBackend>> {
        let mut guard = self.inner.lock().await;
        if guard.backend.is_none() {
            guard.backend = Some(Arc::new(PublisherBackend::connect(url).await?));
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

impl PublisherBackend {
    async fn connect(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url).context("parse moqt url")?;
        match parsed.scheme() {
            "moqt" => Ok(Self::Quic(
                ConnectedPublisher::<QUIC>::connect(&parsed).await?,
            )),
            "https" => Ok(Self::WebTransport(
                ConnectedPublisher::<WEBTRANSPORT>::connect(&parsed).await?,
            )),
            scheme => bail!("unsupported moqt url scheme: {scheme}"),
        }
    }

    async fn setup_namespace(&self, namespace: &[String]) -> Result<()> {
        match self {
            Self::Quic(publisher) => publisher.setup_namespace(namespace).await,
            Self::WebTransport(publisher) => publisher.setup_namespace(namespace).await,
        }
    }

    async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        rotate_group: bool,
        payload: Vec<u8>,
    ) -> Result<()> {
        match self {
            Self::Quic(publisher) => {
                publisher
                    .send_object(namespace, track_name, rotate_group, payload)
                    .await
            }
            Self::WebTransport(publisher) => {
                publisher
                    .send_object(namespace, track_name, rotate_group, payload)
                    .await
            }
        }
    }

    async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        match self {
            Self::Quic(publisher) => {
                publisher
                    .update_video_catalog(namespace, track_name, info)
                    .await
            }
            Self::WebTransport(publisher) => {
                publisher
                    .update_video_catalog(namespace, track_name, info)
                    .await
            }
        }
    }

    async fn update_audio_catalog(
        &self,
        namespace: &[String],
        sample_rate: u32,
        channels: u8,
    ) -> Result<()> {
        match self {
            Self::Quic(publisher) => {
                publisher
                    .update_audio_catalog(namespace, sample_rate, channels)
                    .await
            }
            Self::WebTransport(publisher) => {
                publisher
                    .update_audio_catalog(namespace, sample_rate, channels)
                    .await
            }
        }
    }
}

impl<T: TransportProtocol> ConnectedPublisher<T> {
    async fn connect(url: &url::Url) -> Result<Self> {
        let endpoint = Endpoint::<T>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: false,
            authorization_token: None,
        })?;
        let connecting = endpoint
            .connect(url.as_str())
            .await
            .context("connect moqt transport")?;
        let session = Arc::new(connecting.await.context("establish moqt session")?);

        let state = Arc::new(Mutex::new(BackendState::default()));
        let event_task = Self::spawn_event_loop(session.clone(), state.clone());

        Ok(Self {
            session,
            state,
            event_task,
        })
    }

    fn spawn_event_loop(
        session: Arc<Session<T>>,
        state: Arc<Mutex<BackendState<T>>>,
    ) -> tokio::task::JoinHandle<()> {
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
                        let namespace = handler.track_namespace.clone();
                        let track_name = handler.track_name.clone();
                        let supported = {
                            let guard = state.lock().await;
                            is_supported_track(&track_name, guard.catalogs.get(&namespace))
                        };
                        if !supported {
                            if let Err(err) = handler
                                .error(0, format!("unsupported track: {track_name}"))
                                .await
                            {
                                tracing::warn!(%namespace, %track_name, ?err, "failed to reject SUBSCRIBE");
                            }
                            continue;
                        }

                        let track_alias = match handler.ok(1_000_000, ContentExists::False).await {
                            Ok(track_alias) => track_alias,
                            Err(err) => {
                                tracing::warn!(%namespace, %track_name, ?err, "failed to accept SUBSCRIBE");
                                continue;
                            }
                        };

                        let request_id = handler.request_id();
                        let publication = handler.into_subscription(track_alias);
                        let should_send_catalog = track_name == CATALOG_TRACK_NAME;
                        let writer = TrackWriter::new(
                            session.publisher().create_stream(&publication),
                            now_unix().as_micros() as u64,
                        );
                        let mut guard = state.lock().await;
                        guard.catalogs.entry(namespace.clone()).or_default();
                        let key = (namespace.clone(), track_name.clone());
                        guard.tracks.insert(key.clone(), Some(writer));
                        guard.subscribed_tracks.insert(request_id, key);
                        drop(guard);
                        tracing::info!(%namespace, %track_name, track_alias, "SUBSCRIBE accepted");
                        if should_send_catalog
                            && let Err(err) = Self::send_catalog_snapshot(&state, &namespace).await
                        {
                            tracing::warn!(%namespace, ?err, "failed to send initial catalog");
                        }
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        let request_id = handler.subscribe_id();
                        let mut guard = state.lock().await;
                        match guard.subscribed_tracks.remove(&request_id) {
                            Some(key) => {
                                guard.tracks.remove(&key);
                                tracing::info!(request_id, namespace = %key.0, track_name = %key.1, "UNSUBSCRIBE received; track released");
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
                        if let Err(err) = handler
                            .error(
                                FETCH_NOT_SUPPORTED,
                                "live ingest does not serve FETCH".to_string(),
                            )
                            .await
                        {
                            tracing::warn!(request_id, ?err, "failed to reject FETCH");
                        }
                    }
                    event @ (SessionEvent::GoAway(_)
                    | SessionEvent::MaxRequestId(_)
                    | SessionEvent::RequestsBlocked(_)
                    | SessionEvent::PublishNamespaceCancel(_)
                    | SessionEvent::PublishDone(_)
                    | SessionEvent::SubscribeUpdate(_)
                    | SessionEvent::FetchCancel(_)
                    | SessionEvent::TrackStatus(_)) => {
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
        rotate_group: bool,
        payload: Vec<u8>,
    ) -> Result<()> {
        let key = (namespace.join("/"), track_name.to_string());
        let mut writer = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            match guard.tracks.get_mut(&key).and_then(Option::take) {
                Some(writer) => writer,
                None => return Ok(()),
            }
        };
        let result = write_object(&mut writer, rotate_group, payload).await;
        if let Err(error) = &result
            && is_stopped_by_peer(error)
        {
            tracing::info!(namespace = %key.0, track_name = %key.1, "subscriber stopped the track");
            self.state.lock().await.tracks.remove(&key);
            return Ok(());
        }
        if let Some(slot) = self.state.lock().await.tracks.get_mut(&key) {
            *slot = Some(writer);
        }
        result
    }

    async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        let namespace_path = namespace.join("/");

        let should_send = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            let changed = metadata.video_tracks.get(track_name) != Some(&info);
            if changed {
                metadata.video_tracks.insert(track_name.to_string(), info);
            }
            changed
                && matches!(
                    guard
                        .tracks
                        .get(&(namespace_path.clone(), CATALOG_TRACK_NAME.to_string())),
                    Some(Some(_))
                )
        };

        if should_send {
            Self::send_catalog_snapshot(&self.state, &namespace_path).await?;
        }

        Ok(())
    }

    async fn update_audio_catalog(
        &self,
        namespace: &[String],
        sample_rate: u32,
        channels: u8,
    ) -> Result<()> {
        let namespace_path = namespace.join("/");

        let should_send = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            let changed = metadata.audio_sample_rate != Some(sample_rate)
                || metadata.audio_channels != Some(channels);
            if changed {
                metadata.audio_sample_rate = Some(sample_rate);
                metadata.audio_channels = Some(channels);
            }
            changed
                && matches!(
                    guard
                        .tracks
                        .get(&(namespace_path.clone(), CATALOG_TRACK_NAME.to_string())),
                    Some(Some(_))
                )
        };

        if should_send {
            Self::send_catalog_snapshot(&self.state, &namespace_path).await?;
        }

        Ok(())
    }

    async fn send_catalog_snapshot(
        state: &Arc<Mutex<BackendState<T>>>,
        namespace_path: &str,
    ) -> Result<()> {
        let key = (namespace_path.to_string(), CATALOG_TRACK_NAME.to_string());
        let mut guard = state.lock().await;
        if guard.disconnected {
            bail!("MoQ publisher disconnected");
        }
        let metadata = guard
            .catalogs
            .get(namespace_path)
            .cloned()
            .unwrap_or_default();
        let payload = build_catalog_payload(namespace_path, &metadata)?;
        tracing::debug!(namespace = %namespace_path, catalog = %String::from_utf8_lossy(&payload), "catalog snapshot");
        let Some(Some(writer)) = guard.tracks.get_mut(&key) else {
            return Ok(());
        };
        writer
            .write_group(Bytes::from(payload))
            .await
            .context("send catalog group")?;
        tracing::info!(namespace = %namespace_path, groups = writer.groups(), "catalog sent");
        Ok(())
    }
}

impl<T: TransportProtocol> Drop for ConnectedPublisher<T> {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

async fn write_object<T: TransportProtocol>(
    writer: &mut TrackWriter<T>,
    rotate_group: bool,
    payload: Vec<u8>,
) -> Result<()> {
    if rotate_group || writer.groups() == 0 {
        writer.start_group().await.context("start group")?;
    }
    writer
        .write(Bytes::from(payload), Vec::new())
        .await
        .context("send subgroup object")
}

fn is_supported_track(track_name: &str, metadata: Option<&CatalogMetadata>) -> bool {
    matches!(
        track_name,
        AUDIO_TRACK_NAME | CATALOG_TRACK_NAME | CHAT_TRACK_NAME
    ) || metadata.is_some_and(|metadata| metadata.video_tracks.contains_key(track_name))
}

fn build_catalog_payload(namespace_path: &str, metadata: &CatalogMetadata) -> Result<Vec<u8>> {
    let namespace = Some(namespace_path.to_string());
    let depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        AUDIO_TRACK_NAME.to_string(),
    ]);

    let alt_group = (metadata.video_tracks.len() > 1).then_some(1);
    let mut tracks: Vec<Track> = metadata
        .video_tracks
        .iter()
        .map(|(name, info)| Track {
            namespace: namespace.clone(),
            name: name.clone(),
            packaging: Packaging::Known(KnownPackaging::Loc),
            event_type: None,
            role: Some(TrackRole::Known(KnownTrackRole::Video)),
            is_live: true,
            target_latency: None,
            label: Some(info.label.clone()),
            render_group: None,
            alt_group,
            init_data: None,
            depends: None,
            temporal_id: None,
            spatial_id: None,
            codec: Some(info.codec.clone()),
            mime_type: Some("video/h264".to_string()),
            framerate: Some(30.0),
            timescale: None,
            bitrate: None,
            width: Some(info.width),
            height: Some(info.height),
            sample_rate: None,
            channel_config: None,
            display_width: None,
            display_height: None,
            lang: None,
            parent_name: None,
            track_duration: None,
        })
        .collect();
    tracks.extend([
        Track {
            namespace: namespace.clone(),
            name: AUDIO_TRACK_NAME.to_string(),
            packaging: Packaging::Known(KnownPackaging::Loc),
            event_type: None,
            role: Some(TrackRole::Known(KnownTrackRole::Audio)),
            is_live: true,
            target_latency: None,
            label: Some("Audio".to_string()),
            render_group: None,
            alt_group: None,
            init_data: None,
            depends: None,
            temporal_id: None,
            spatial_id: None,
            codec: Some("mp4a.40.2".to_string()),
            mime_type: Some("audio/aac".to_string()),
            framerate: None,
            timescale: None,
            bitrate: None,
            width: None,
            height: None,
            sample_rate: metadata.audio_sample_rate,
            channel_config: metadata.audio_channels.map(channel_config_label),
            display_width: None,
            display_height: None,
            lang: None,
            parent_name: None,
            track_duration: None,
        },
        Track {
            namespace,
            name: CHAT_TRACK_NAME.to_string(),
            packaging: Packaging::Known(KnownPackaging::EventTimeline),
            event_type: Some(CHAT_EVENT_TYPE.to_string()),
            role: Some(TrackRole::Other(CHAT_TRACK_NAME.to_string())),
            is_live: true,
            target_latency: None,
            label: Some("Chat".to_string()),
            render_group: None,
            alt_group: None,
            init_data: None,
            depends,
            temporal_id: None,
            spatial_id: None,
            codec: None,
            mime_type: Some("application/json".to_string()),
            framerate: None,
            timescale: None,
            bitrate: None,
            width: None,
            height: None,
            sample_rate: None,
            channel_config: None,
            display_width: None,
            display_height: None,
            lang: None,
            parent_name: None,
            track_duration: None,
        },
    ]);

    let catalog = Catalog {
        version: Some(1),
        delta_update: None,
        add_tracks: None,
        remove_tracks: None,
        clone_tracks: None,
        generated_at: Some(now_unix().as_millis() as u64),
        is_complete: Some(true),
        tracks: Some(tracks),
    };

    serde_json::to_vec(&catalog).context("serialize msf catalog")
}

fn channel_config_label(channels: u8) -> String {
    match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}ch"),
    }
}

fn is_stopped_by_peer(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<TransportSendError>(),
            Some(TransportSendError::Stopped { .. } | TransportSendError::InvalidStopped { .. })
        )
    })
}

pub(crate) fn now_unix() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stop_sending_through_context_layers() {
        // Arrange
        let stopped = anyhow::Error::new(TransportSendError::InvalidStopped { code: 0 })
            .context("send subgroup object");
        let lost = anyhow::Error::new(TransportSendError::ConnectionLost {
            reason: "timeout".into(),
        })
        .context("send subgroup object");

        // Act / Assert
        assert!(is_stopped_by_peer(&stopped));
        assert!(!is_stopped_by_peer(&lost));
    }
}

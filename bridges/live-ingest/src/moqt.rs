use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose};
use bytes::Bytes;
use media_streaming_format::{
    Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};
use mediapack::{
    aac::AudioSpecificConfig, h264::AvcDecoderConfigurationRecord, mp4::Fmp4TrackMuxer,
};
use moqt::{
    ClientConfig, ContentExists, Endpoint, ExtensionHeaders, QUIC, Session, SessionEvent,
    TrackWriter, TransportProtocol, TransportSendError, WEBTRANSPORT,
};
use tokio::sync::Mutex;

pub(crate) const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
const CATALOG_TRACK_NAME: &str = "catalog";
pub(crate) const TIMELINE_TRACK_NAME: &str = "timeline";
const CMAF_TRACK_SUFFIX: &str = "_cmaf";
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

pub(crate) fn cmaf_track_name(track_name: &str) -> String {
    format!("{track_name}{CMAF_TRACK_SUFFIX}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CatalogMetadata {
    video_tracks: BTreeMap<String, VideoTrackInfo>,
    audio_config: Option<AudioSpecificConfig>,
}

pub(crate) struct OutgoingObject {
    pub(crate) group: GroupBoundary,
    pub(crate) extension_headers: ExtensionHeaders,
    pub(crate) payload: Bytes,
}

impl OutgoingObject {
    pub(crate) fn plain(group: GroupBoundary, payload: Bytes) -> Self {
        Self {
            group,
            extension_headers: ExtensionHeaders::default(),
            payload,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GroupBoundary {
    /// Only inside an open group; the object is dropped when there is none,
    /// because a group must not start on it.
    Within,
    /// Inside the open group, or the first object of a new one when there is
    /// none.
    Join,
    Next,
    At(u64),
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
        object: OutgoingObject,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
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
        config: AudioSpecificConfig,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
            .await?
            .update_audio_catalog(namespace, config)
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
        object: OutgoingObject,
    ) -> Result<()> {
        match self {
            Self::Quic(publisher) => publisher.send_object(namespace, track_name, object).await,
            Self::WebTransport(publisher) => {
                publisher.send_object(namespace, track_name, object).await
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
        config: AudioSpecificConfig,
    ) -> Result<()> {
        match self {
            Self::Quic(publisher) => publisher.update_audio_catalog(namespace, config).await,
            Self::WebTransport(publisher) => {
                publisher.update_audio_catalog(namespace, config).await
            }
        }
    }
}

impl<T: TransportProtocol> ConnectedPublisher<T> {
    async fn connect(url: &url::Url) -> Result<Self> {
        let endpoint = Endpoint::<T>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: false,
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
                        let mut guard = state.lock().await;
                        guard.catalogs.entry(namespace.clone()).or_default();
                        let key = (namespace.clone(), track_name.clone());
                        let first_group_id = guard
                            .tracks
                            .get(&key)
                            .and_then(|slot| slot.as_ref())
                            .map(TrackWriter::next_group_id)
                            .unwrap_or_else(|| now_unix().as_micros() as u64);
                        let writer = TrackWriter::new(
                            session.publisher().create_stream(&publication),
                            first_group_id,
                        );
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
        object: OutgoingObject,
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
        let result = write_object(&mut writer, object).await;
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
        config: AudioSpecificConfig,
    ) -> Result<()> {
        let namespace_path = namespace.join("/");

        let should_send = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            let changed = metadata.audio_config.as_ref() != Some(&config);
            if changed {
                metadata.audio_config = Some(config);
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
    object: OutgoingObject,
) -> Result<()> {
    match object.group {
        GroupBoundary::Within if writer.groups() == 0 => return Ok(()),
        GroupBoundary::Within => {}
        GroupBoundary::Join if writer.groups() > 0 => {}
        GroupBoundary::Join | GroupBoundary::Next => {
            writer.start_group().await.context("start group")?;
        }
        GroupBoundary::At(group_id) => {
            writer
                .start_group_at(group_id)
                .await
                .context("start group at the aligned id")?;
        }
    }
    writer
        .write_with_extension_headers(object.payload, object.extension_headers)
        .await
        .context("send subgroup object")
}

fn is_supported_track(track_name: &str, metadata: Option<&CatalogMetadata>) -> bool {
    let media_track = track_name
        .strip_suffix(CMAF_TRACK_SUFFIX)
        .unwrap_or(track_name);
    matches!(
        track_name,
        CATALOG_TRACK_NAME | CHAT_TRACK_NAME | TIMELINE_TRACK_NAME
    ) || media_track == AUDIO_TRACK_NAME
        || metadata.is_some_and(|metadata| metadata.video_tracks.contains_key(media_track))
}

fn build_catalog_payload(namespace_path: &str, metadata: &CatalogMetadata) -> Result<Vec<u8>> {
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

/// draft-ietf-moq-cmsf-01 §3.1 puts the init segment in `initData` and §3.5.2
/// declares every group and object starts on a SAP type 1 sample.
fn cmaf_sibling(loc: &Track, mime_type: &str, init_data: String, alt_group: Option<u64>) -> Track {
    Track {
        name: cmaf_track_name(&loc.name),
        packaging: Packaging::Known(KnownPackaging::Cmaf),
        label: loc.label.as_ref().map(|label| format!("{label} (CMAF)")),
        alt_group,
        mime_type: Some(mime_type.to_string()),
        init_data: Some(init_data),
        max_grp_sap_starting_type: Some(1),
        max_obj_sap_starting_type: Some(1),
        ..loc.clone()
    }
}

fn track(
    namespace: &Option<String>,
    name: &str,
    packaging: KnownPackaging,
    role: TrackRole,
) -> Track {
    Track {
        namespace: namespace.clone(),
        name: name.to_string(),
        packaging: Packaging::Known(packaging),
        event_type: None,
        role: Some(role),
        is_live: true,
        target_latency: None,
        label: None,
        render_group: None,
        alt_group: None,
        init_data: None,
        depends: None,
        temporal_id: None,
        spatial_id: None,
        codec: None,
        mime_type: None,
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
        max_grp_sap_starting_type: None,
        max_obj_sap_starting_type: None,
    }
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
    fn accepts_cmaf_siblings_of_media_tracks() {
        // Arrange
        let metadata = metadata_with_video_and_audio();

        // Act / Assert
        assert!(is_supported_track("video_cmaf", Some(&metadata)));
        assert!(is_supported_track("audio_cmaf", Some(&metadata)));
        assert!(!is_supported_track("chat_cmaf", Some(&metadata)));
        assert!(!is_supported_track("video_720p_cmaf", Some(&metadata)));
    }

    #[test]
    fn catalog_pairs_each_media_track_with_a_cmaf_track_carrying_its_init_segment() {
        // Arrange
        let metadata = metadata_with_video_and_audio();

        // Act
        let payload = build_catalog_payload("live/test", &metadata).unwrap();
        let catalog: Catalog = serde_json::from_slice(&payload).unwrap();

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

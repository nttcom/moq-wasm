use std::{
    collections::{HashMap, HashSet},
    net::ToSocketAddrs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use media_streaming_format::{
    Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole},
};
use moqt::{
    ClientConfig, ContentExists, Endpoint, ExtensionHeaders, PublishedResource, QUIC, Session,
    SessionEvent, SubgroupId, SubgroupObject, SubgroupObjectSender, TransportProtocol,
    WEBTRANSPORT,
};
use tokio::sync::Mutex;

const VIDEO_TRACK_NAME: &str = "video";
const AUDIO_TRACK_NAME: &str = "audio";
const CATALOG_TRACK_NAME: &str = "catalog";
const CHAT_TRACK_NAME: &str = "chat";
const CHAT_EVENT_TYPE: &str = "com.skyway.chat.v1";
const JS_COMPAT_OBJECT_ID_DELTA: u64 = 1;

#[derive(Clone)]
pub struct MoqtManager {
    url: Option<String>,
    inner: Arc<Mutex<ManagerState>>,
}

#[derive(Default)]
struct ManagerState {
    backend: Option<Arc<PublisherBackend>>,
}

struct TrackInfo<T: TransportProtocol> {
    publication: Option<PublishedResource>,
    stream: Option<SubgroupObjectSender<T>>,
    object_id: u64,
    group_id: u64,
}

impl<T: TransportProtocol> Default for TrackInfo<T> {
    fn default() -> Self {
        Self {
            publication: None,
            stream: None,
            object_id: 0,
            group_id: 0,
        }
    }
}

struct BackendState<T: TransportProtocol> {
    announced_namespaces: HashSet<String>,
    tracks: HashMap<(String, String), TrackInfo<T>>,
    catalogs: HashMap<String, CatalogMetadata>,
    disconnected: bool,
}

impl<T: TransportProtocol> Default for BackendState<T> {
    fn default() -> Self {
        Self {
            announced_namespaces: HashSet::new(),
            tracks: HashMap::new(),
            catalogs: HashMap::new(),
            disconnected: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CatalogMetadata {
    video_codec: Option<String>,
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
        payload: &[u8],
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
        codec: Option<&str>,
    ) -> Result<()> {
        let url = match &self.url {
            Some(u) => u.clone(),
            None => return Ok(()),
        };

        self.ensure_backend(&url)
            .await?
            .update_video_catalog(namespace, codec)
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
        payload: &[u8],
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

    async fn update_video_catalog(&self, namespace: &[String], codec: Option<&str>) -> Result<()> {
        match self {
            Self::Quic(publisher) => publisher.update_video_catalog(namespace, codec).await,
            Self::WebTransport(publisher) => publisher.update_video_catalog(namespace, codec).await,
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
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("missing host in moqt url"))?;
        let port = match url.scheme() {
            "moqt" => url.port().unwrap_or(4433),
            "https" => url.port().unwrap_or(443),
            scheme => bail!("unsupported scheme for transport: {scheme}"),
        };
        let remote_address = (host, port)
            .to_socket_addrs()
            .context("resolve moqt address")?
            .next()
            .ok_or_else(|| anyhow!("failed to resolve moqt address"))?;
        let endpoint = Endpoint::<T>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: false,
        })?;
        let connecting = endpoint
            .connect(remote_address, host)
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
                        eprintln!("[moqt] event loop ended: {err:?}");
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                };

                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        if let Err(err) = handler.ok().await {
                            eprintln!(
                                "[moqt] failed to ack publish_namespace ns={}: {err:?}",
                                handler.track_namespace
                            );
                        }
                    }
                    SessionEvent::SubscribeNameSpace(handler) => {
                        if let Err(err) = handler.ok().await {
                            eprintln!(
                                "[moqt] failed to ack subscribe_namespace prefix={}: {err:?}",
                                handler.track_namespace_prefix
                            );
                        }
                    }
                    SessionEvent::Subscribe(handler) => {
                        let namespace = handler.track_namespace.clone();
                        let track_name = handler.track_name.clone();
                        if !is_supported_track(&track_name) {
                            if let Err(err) = handler
                                .error(0, format!("unsupported track: {track_name}"))
                                .await
                            {
                                eprintln!(
                                    "[moqt] failed to reject subscribe ns={} track={}: {err:?}",
                                    namespace, track_name
                                );
                            }
                            continue;
                        }

                        let track_alias = match handler.ok(1_000_000, ContentExists::False).await {
                            Ok(track_alias) => track_alias,
                            Err(err) => {
                                eprintln!(
                                    "[moqt] failed to accept subscribe ns={} track={}: {err:?}",
                                    namespace, track_name
                                );
                                continue;
                            }
                        };

                        let publication = handler.into_publication(track_alias);
                        let should_send_catalog = track_name == CATALOG_TRACK_NAME;
                        let mut guard = state.lock().await;
                        guard.catalogs.entry(namespace.clone()).or_default();
                        let entry = guard
                            .tracks
                            .entry((namespace.clone(), track_name.clone()))
                            .or_default();
                        entry.publication = Some(publication);
                        entry.stream = None;
                        entry.object_id = 0;
                        entry.group_id = 0;
                        drop(guard);
                        println!(
                            "[moqt] subscribe accepted ns={} track={} alias={}",
                            namespace, track_name, track_alias
                        );
                        if should_send_catalog
                            && let Err(err) =
                                Self::send_catalog_snapshot(&session, &state, &namespace).await
                        {
                            eprintln!(
                                "[moqt] failed to send initial catalog ns={}: {err:?}",
                                namespace
                            );
                        }
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        println!("[moqt] unsubscribe received id={}", handler.subscribe_id());
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => {
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                    SessionEvent::Publish(handler) => {
                        eprintln!(
                            "[moqt] unexpected publish request ns={} track={}",
                            handler.track_namespace, handler.track_name
                        );
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
            println!("[moqt] namespace published ns={namespace_path}");
        }
        Ok(())
    }

    async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        rotate_group: bool,
        payload: &[u8],
    ) -> Result<()> {
        let namespace_path = namespace.join("/");
        let key = (namespace_path.clone(), track_name.to_string());
        let (publication, mut stream, mut object_id, mut group_id) = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let entry = guard
                .tracks
                .get_mut(&key)
                .ok_or_else(|| anyhow!("track not set up: {namespace_path}/{track_name}"))?;
            (
                entry.publication.clone().ok_or_else(|| {
                    anyhow!("subscribe not completed: {namespace_path}/{track_name}")
                })?,
                entry.stream.take(),
                entry.object_id,
                entry.group_id,
            )
        };

        let send_result: Result<()> = async {
            if rotate_group {
                if let Some(mut current_stream) = stream.take() {
                    let eog = current_stream.create_object_field(
                        JS_COMPAT_OBJECT_ID_DELTA,
                        empty_extension_headers(),
                        SubgroupObject::new_status(moqt::wire::ObjectStatus::EndOfGroup as u64),
                    );
                    current_stream
                        .send(eog)
                        .await
                        .context("send end-of-group object")?;
                    if let Err(err) = current_stream.close().await {
                        eprintln!("[moqt] failed to close previous subgroup stream: {err:?}");
                    }
                    group_id = group_id.saturating_add(1);
                    object_id = 0;
                }
            }

            if stream.is_none() {
                let uninit_stream = self
                    .session
                    .publisher()
                    .create_stream(&publication)
                    .next()
                    .await
                    .context("open subgroup stream")?;
                let header =
                    uninit_stream.create_header(group_id, SubgroupId::None, 0, false, false);
                stream = Some(
                    uninit_stream
                        .send_header(header)
                        .await
                        .context("send subgroup header")?,
                );
                println!(
                    "[moqt] subgroup header sent ns={} track={} group_id={}",
                    namespace_path, track_name, group_id
                );
            }

            let stream_ref = stream
                .as_mut()
                .ok_or_else(|| anyhow!("subgroup stream not initialized"))?;
            let object = stream_ref.create_object_field(
                JS_COMPAT_OBJECT_ID_DELTA,
                empty_extension_headers(),
                SubgroupObject::new_payload(payload.to_vec().into()),
            );
            stream_ref
                .send(object)
                .await
                .context("send subgroup object")?;
            println!(
                "[moqt] subgroup object sent ns={} track={} group_id={} object_id={}",
                namespace_path, track_name, group_id, object_id
            );
            object_id = object_id.saturating_add(1);
            Ok(())
        }
        .await;

        let mut guard = self.state.lock().await;
        if let Some(entry) = guard.tracks.get_mut(&key) {
            if send_result.is_ok() {
                entry.object_id = object_id;
                entry.group_id = group_id;
            }
            entry.stream = stream;
        }
        send_result
    }

    async fn update_video_catalog(&self, namespace: &[String], codec: Option<&str>) -> Result<()> {
        let namespace_path = namespace.join("/");
        let normalized_codec = codec
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        let should_send = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            let changed = metadata.video_codec != normalized_codec;
            if changed {
                metadata.video_codec = normalized_codec;
            }
            changed
                && guard
                    .tracks
                    .get(&(namespace_path.clone(), CATALOG_TRACK_NAME.to_string()))
                    .and_then(|entry| entry.publication.as_ref())
                    .is_some()
        };

        if should_send {
            Self::send_catalog_snapshot(&self.session, &self.state, &namespace_path).await?;
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
                && guard
                    .tracks
                    .get(&(namespace_path.clone(), CATALOG_TRACK_NAME.to_string()))
                    .and_then(|entry| entry.publication.as_ref())
                    .is_some()
        };

        if should_send {
            Self::send_catalog_snapshot(&self.session, &self.state, &namespace_path).await?;
        }

        Ok(())
    }

    async fn send_catalog_snapshot(
        session: &Arc<Session<T>>,
        state: &Arc<Mutex<BackendState<T>>>,
        namespace_path: &str,
    ) -> Result<()> {
        let key = (namespace_path.to_string(), CATALOG_TRACK_NAME.to_string());
        let (publication, group_id, payload) = {
            let mut guard = state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard
                .catalogs
                .get(namespace_path)
                .cloned()
                .unwrap_or_default();
            let entry = match guard.tracks.get_mut(&key) {
                Some(entry) => entry,
                None => return Ok(()),
            };
            let publication = match entry.publication.clone() {
                Some(publication) => publication,
                None => return Ok(()),
            };
            let payload = build_catalog_payload(namespace_path, &metadata)?;
            let group_id = entry.group_id;
            entry.group_id = entry.group_id.saturating_add(1);
            entry.object_id = 0;
            entry.stream = None;
            (publication, group_id, payload)
        };

        let uninit_stream = session
            .publisher()
            .create_stream(&publication)
            .next()
            .await
            .context("open catalog stream")?;
        let header = uninit_stream.create_header(group_id, SubgroupId::None, 0, false, false);
        let mut stream = uninit_stream
            .send_header(header)
            .await
            .context("send catalog subgroup header")?;
        let object = stream.create_object_field(
            JS_COMPAT_OBJECT_ID_DELTA,
            empty_extension_headers(),
            SubgroupObject::new_payload(payload.into()),
        );
        stream
            .send(object)
            .await
            .context("send catalog subgroup object")?;
        stream
            .close()
            .await
            .context("close catalog subgroup stream")?;
        println!(
            "[moqt] catalog sent ns={} group_id={}",
            namespace_path, group_id
        );
        Ok(())
    }
}

impl<T: TransportProtocol> Drop for ConnectedPublisher<T> {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

fn empty_extension_headers() -> ExtensionHeaders {
    ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions: vec![],
    }
}

fn is_supported_track(track_name: &str) -> bool {
    matches!(
        track_name,
        VIDEO_TRACK_NAME | AUDIO_TRACK_NAME | CATALOG_TRACK_NAME | CHAT_TRACK_NAME
    )
}

fn build_catalog_payload(namespace_path: &str, metadata: &CatalogMetadata) -> Result<Vec<u8>> {
    let namespace = Some(namespace_path.to_string());
    let depends = Some(vec![
        VIDEO_TRACK_NAME.to_string(),
        AUDIO_TRACK_NAME.to_string(),
    ]);

    let tracks = vec![
        Track {
            namespace: namespace.clone(),
            name: VIDEO_TRACK_NAME.to_string(),
            packaging: Packaging::Known(KnownPackaging::Loc),
            event_type: None,
            role: Some(TrackRole::Known(KnownTrackRole::Video)),
            is_live: true,
            target_latency: None,
            label: Some("Video".to_string()),
            render_group: None,
            alt_group: None,
            init_data: None,
            depends: None,
            temporal_id: None,
            spatial_id: None,
            codec: metadata.video_codec.clone(),
            mime_type: Some("video/h264".to_string()),
            framerate: Some(30.0),
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
    ];

    let catalog = Catalog {
        version: Some(1),
        delta_update: None,
        add_tracks: None,
        remove_tracks: None,
        clone_tracks: None,
        generated_at: Some(now_unix_ms()),
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

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

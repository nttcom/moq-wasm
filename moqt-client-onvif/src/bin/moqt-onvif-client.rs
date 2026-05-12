use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use clap::Parser;
use media_streaming_format::{
    Catalog, KnownPackaging, KnownTrackRole, Packaging, Track, TrackRole,
};
use moqt::{
    ClientConfig, ContentExists, DataReceiver, Endpoint, ExtensionHeaders, FilterType, GroupOrder,
    ObjectDatagramPayload, PublishedResource, Session, SessionEvent, SubgroupId, SubgroupObject,
    SubgroupObjectSender, SubscribeOption, WEBTRANSPORT,
};
use moqt_client_onvif::{
    app_config, cli, onvif_client, onvif_profile_list, onvif_stream_uri, ptz_worker, rtsp_decoder,
    rtsp_frame::EncodedPacket, soap_client,
};
use packages::loc::{CaptureTimestamp, LocHeader, LocHeaderExtension, VideoConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use url::Url;

const LOC_HEADER_SENTINEL: &[u8] = b"loc:";

#[derive(Parser, Debug)]
#[command(author, version, about = "Bridge ONVIF PTZ and RTSP video over MoQ")]
struct MoqtArgs {
    #[command(flatten)]
    onvif: cli::Args,

    /// MoQ WebTransport URL (e.g. https://localhost:4433)
    #[arg(long)]
    moqt_url: String,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long, default_value_t = false)]
    insecure_skip_tls_verify: bool,

    /// Track namespace for publishing video (slash-separated)
    #[arg(long, default_value = "onvif/client")]
    publish_namespace: String,

    /// Track namespace for subscribing to commands (slash-separated)
    #[arg(long, default_value = "onvif/viewer")]
    subscribe_namespace: String,

    /// Track name prefix for video streams
    #[arg(long, default_value = "video")]
    video_track: String,

    /// Track name for profile catalog
    #[arg(long, default_value = "catalog")]
    catalog_track: String,

    /// Track name for ONVIF commands
    #[arg(long, default_value = "command")]
    command_track: String,

    /// Subscriber priority for command track
    #[arg(long, default_value_t = 0)]
    subscriber_priority: u8,

    /// Publisher priority for video stream
    #[arg(long, default_value_t = 0)]
    publisher_priority: u8,

    /// Codec string advertised in video metadata
    #[arg(long, default_value = "avc1.640028")]
    video_codec: String,

    /// Payload format to send over MoQ: annexb or avcc
    #[arg(long, default_value = "avcc")]
    payload_format: String,

    /// Dump the first keyframe AnnexB payload for ffprobe (default: /tmp/moqt-onvif-keyframe.h264)
    #[arg(
        long,
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "/tmp/moqt-onvif-keyframe.h264"
    )]
    dump_keyframe: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger();
    let args = MoqtArgs::parse();
    let target = app_config::Target::from_args(&args.onvif)?;
    let publish_namespace = parse_namespace(&args.publish_namespace);
    if publish_namespace.is_empty() {
        bail!("publish namespace is required");
    }
    let subscribe_namespace = parse_namespace(&args.subscribe_namespace);
    if subscribe_namespace.is_empty() {
        bail!("subscribe namespace is required");
    }

    let controller = ptz_worker::Controller::new(target.clone())?;
    let command_sender = controller.command_sender();
    spawn_ptz_error_logger(controller);

    let profile_tracks = fetch_profile_tracks(&target, &args.video_track).await?;
    if profile_tracks.is_empty() {
        bail!("no video profiles available");
    }
    let catalog_track = args.catalog_track.clone();
    let expected_tracks = build_expected_tracks(&catalog_track, &profile_tracks);

    let wt_connect_started = Instant::now();
    let session = connect_session(&args.moqt_url, args.insecure_skip_tls_verify)
        .await
        .context("connect moqt session")?;
    let publisher = session.publisher();
    log::info!(
        "WebTransport connected: url={} elapsed_ms={}",
        args.moqt_url,
        wt_connect_started.elapsed().as_millis()
    );
    publisher
        .publish_namespace(publish_namespace.join("/"))
        .await
        .context("publisher publish_namespace")?;

    let mut catalog_state = CatalogUpdateState::new();
    let selection_context = VideoSelectionContext {
        session: session.as_ref(),
        publisher: &publisher,
        namespace: &publish_namespace,
        catalog_track: &catalog_track,
        expected_tracks: &expected_tracks,
        profile_tracks: &profile_tracks,
        publisher_priority: args.publisher_priority,
    };
    let (selected_profile, video_publication) =
        wait_for_video_selection(selection_context, &mut catalog_state)
            .await
            .context("wait for video subscribe")?;

    let mut command_subscriber = session.subscriber();
    let command_subscription = command_subscriber
        .subscribe(
            subscribe_namespace.join("/"),
            args.command_track.clone(),
            SubscribeOption {
                subscriber_priority: args.subscriber_priority,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::NextGroupStart,
            },
        )
        .await
        .context("subscribe command track")?;
    spawn_command_receiver(
        command_subscriber,
        command_subscription,
        command_sender.clone(),
    );

    let (encoded_tx, encoded_rx) = mpsc::channel(16);
    let (rtsp_err_tx, rtsp_err_rx) = std_mpsc::channel();
    let codec_label = args.video_codec.clone();
    let payload_format = match rtsp_decoder::PayloadFormat::parse(&args.payload_format) {
        Some(format) => format,
        None => {
            log::warn!(
                "unknown payload format '{}', fallback to annexb",
                args.payload_format
            );
            rtsp_decoder::PayloadFormat::AnnexB
        }
    };
    let rtsp_url = selected_profile.rtsp_url.clone();
    std::thread::spawn(move || {
        rtsp_decoder::run_encoded_url(
            rtsp_url,
            codec_label,
            payload_format,
            encoded_tx,
            rtsp_err_tx,
        );
    });
    spawn_rtsp_error_logger(rtsp_err_rx);

    catalog_state.selected_track = Some(selected_profile.track_name.clone());
    run_moqt_bridge(BridgeContext {
        publisher,
        publication: video_publication,
        rx: encoded_rx,
        publisher_priority: args.publisher_priority,
        dump_keyframe: args.dump_keyframe,
        publish_namespace,
        profile_tracks,
        catalog_state,
    })
    .await?;

    Ok(())
}

struct BridgeContext {
    publisher: moqt::Publisher<WEBTRANSPORT>,
    publication: PublishedResource,
    rx: mpsc::Receiver<EncodedPacket>,
    publisher_priority: u8,
    dump_keyframe: Option<PathBuf>,
    publish_namespace: Vec<String>,
    profile_tracks: Vec<ProfileTrack>,
    catalog_state: CatalogUpdateState,
}

async fn run_moqt_bridge(ctx: BridgeContext) -> Result<()> {
    let BridgeContext {
        publisher,
        publication,
        mut rx,
        publisher_priority,
        dump_keyframe,
        publish_namespace,
        profile_tracks,
        mut catalog_state,
    } = ctx;
    let mut state = VideoStreamState::default();
    let mut dump_state = dump_keyframe.map(KeyframeDump::new);
    while let Some(packet) = rx.recv().await {
        maybe_send_catalog_update(
            &publisher,
            &mut catalog_state,
            &publish_namespace,
            &profile_tracks,
            publisher_priority,
            &packet,
        )
        .await?;
        send_video_packet(
            &publisher,
            &mut state,
            &publication,
            publisher_priority,
            &mut dump_state,
            packet,
        )
        .await?;
    }
    Ok(())
}

async fn send_video_packet(
    publisher: &moqt::Publisher<WEBTRANSPORT>,
    state: &mut VideoStreamState,
    publication: &PublishedResource,
    publisher_priority: u8,
    dump_state: &mut Option<KeyframeDump>,
    packet: EncodedPacket,
) -> Result<()> {
    if let Some(dump_state) = dump_state.as_mut() {
        dump_state.maybe_write(&packet)?;
    }
    let mut pending_group_close: Option<PendingGroupClose> = None;
    if packet.is_keyframe {
        pending_group_close = state
            .start_group(publisher, publication, publisher_priority)
            .await
            .context("send subgroup header")?;
    } else if !state.started {
        log::warn!("skipping non-keyframe packet before first keyframe");
        return Ok(());
    }

    let loc_header = build_loc_header(&packet);
    let extension_headers =
        loc_header_to_extension_headers(&loc_header).context("build loc header extensions")?;
    let payload = serialize_chunk_payload(&packet)?;
    let Some(stream) = state.stream.as_mut() else {
        return Ok(());
    };
    let object = stream.create_object_field(
        0,
        extension_headers,
        SubgroupObject::new_payload(payload.into()),
    );
    stream.send(object).await.context("send subgroup object")?;
    let total_delay_us = now_micros().saturating_sub(packet.ingest_wallclock_micros);
    log::info!(
        "MoQ send video packet group_id={} object_id={} timestamp_us={} total_delay_ms={} keyframe={}",
        state.group_id,
        state.object_id,
        packet.timestamp_us,
        total_delay_us / 1_000,
        packet.is_keyframe
    );
    if let Some(last_ts) = state.last_timestamp_us {
        let delta = packet.timestamp_us.saturating_sub(last_ts);
        if delta >= 200_000 {
            log::warn!(
                "MoQ send video timestamp gap: group_id={} object_id={} delta_us={}",
                state.group_id,
                state.object_id,
                delta
            );
        }
    }
    state.last_timestamp_us = Some(packet.timestamp_us);
    state.object_id += 1;
    if let Some(pending) = pending_group_close {
        spawn_pending_group_close(pending);
    }
    Ok(())
}

#[derive(Serialize)]
struct ChunkMetadata<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    timestamp: u64,
    duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<&'a str>,
    #[serde(rename = "descriptionBase64", skip_serializing_if = "Option::is_none")]
    description_base64: Option<&'a str>,
    #[serde(rename = "avcFormat", skip_serializing_if = "Option::is_none")]
    avc_format: Option<&'a str>,
}

fn serialize_chunk_payload(packet: &EncodedPacket) -> Result<Vec<u8>> {
    let avc_format = match packet.avc_format.as_deref() {
        Some("avcc") => Some("avc"),
        Some("annexb") => Some("annexb"),
        other => other,
    };
    let metadata = ChunkMetadata {
        kind: if packet.is_keyframe { "key" } else { "delta" },
        timestamp: packet.timestamp_us,
        duration: packet.duration_us,
        codec: packet.codec.as_deref(),
        description_base64: packet.description_base64.as_deref(),
        avc_format,
    };
    let meta_bytes = serde_json::to_vec(&metadata).context("serialize chunk metadata")?;
    let mut payload = Vec::with_capacity(4 + meta_bytes.len() + packet.data.len());
    payload.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(&meta_bytes);
    payload.extend_from_slice(&packet.data);
    Ok(payload)
}

async fn send_catalog(
    publisher: &moqt::Publisher<WEBTRANSPORT>,
    publication: &PublishedResource,
    group_id: u64,
    publisher_priority: u8,
    namespace: &[String],
    codec_update: Option<&CatalogCodecUpdate<'_>>,
    profiles: &[ProfileTrack],
) -> Result<()> {
    let tracks = build_catalog_tracks(namespace, codec_update, profiles);
    let catalog = Catalog {
        version: Some(1),
        delta_update: None,
        add_tracks: None,
        remove_tracks: None,
        clone_tracks: None,
        generated_at: Some(now_millis()),
        is_complete: Some(true),
        tracks: Some(tracks),
    };
    let data = serde_json::to_vec(&catalog).context("serialize catalog json")?;
    let uninit_stream = publisher
        .create_stream(publication)
        .next()
        .await
        .context("open catalog subgroup stream")?;
    let header =
        uninit_stream.create_header(group_id, SubgroupId::None, publisher_priority, false, false);
    let mut stream = uninit_stream
        .send_header(header)
        .await
        .context("send catalog subgroup header")?;
    let object = stream.create_object_field(
        0,
        empty_extension_headers(),
        SubgroupObject::new_payload(data.into()),
    );
    stream
        .send(object)
        .await
        .context("send catalog subgroup object")?;
    stream
        .close()
        .await
        .context("close catalog subgroup stream")?;
    if let Some(update) = codec_update {
        log::info!(
            "Catalog sent: tracks={} codec={} track={} group_id={}",
            profiles.len(),
            update.codec,
            update.track_name,
            group_id
        );
    } else {
        log::info!(
            "Catalog sent: tracks={} codec=- group_id={}",
            profiles.len(),
            group_id
        );
    }
    Ok(())
}

struct KeyframeDump {
    path: PathBuf,
    written: bool,
}

impl KeyframeDump {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            written: false,
        }
    }

    fn maybe_write(&mut self, packet: &EncodedPacket) -> Result<()> {
        if self.written || !packet.is_keyframe {
            return Ok(());
        }
        std::fs::write(&self.path, &packet.data).with_context(|| {
            format!("write keyframe payload to {}", self.path.to_string_lossy())
        })?;
        self.written = true;
        log::info!(
            "RTSP keyframe payload dumped for ffprobe: {}",
            self.path.display()
        );
        Ok(())
    }
}

fn handle_command_payload(
    payload: &[u8],
    command_sender: &std_mpsc::Sender<ptz_worker::Command>,
) -> Result<()> {
    let command: CommandPayload = serde_json::from_slice(payload).context("parse command json")?;
    let mapped = command.into_command();
    log::info!("PTZ command received: {}", mapped.label());
    let _ = command_sender.send(mapped.inner);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommandPayload {
    Absolute {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Relative {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Continuous {
        pan: f32,
        tilt: f32,
        zoom: f32,
        speed: f32,
    },
    Stop,
    Center {
        speed: f32,
    },
}

struct MappedCommand {
    inner: ptz_worker::Command,
    label: &'static str,
}

impl MappedCommand {
    fn label(&self) -> &'static str {
        self.label
    }
}

impl CommandPayload {
    fn into_command(self) -> MappedCommand {
        match self {
            CommandPayload::Absolute {
                pan,
                tilt,
                zoom,
                speed,
            } => MappedCommand {
                inner: ptz_worker::Command::Absolute {
                    pan,
                    tilt,
                    zoom,
                    speed,
                },
                label: "AbsoluteMove",
            },
            CommandPayload::Relative {
                pan,
                tilt,
                zoom,
                speed,
            } => MappedCommand {
                inner: ptz_worker::Command::Relative {
                    pan,
                    tilt,
                    zoom,
                    speed,
                },
                label: "RelativeMove",
            },
            CommandPayload::Continuous {
                pan,
                tilt,
                zoom,
                speed,
            } => MappedCommand {
                inner: ptz_worker::Command::Continuous {
                    pan,
                    tilt,
                    zoom,
                    speed,
                },
                label: "ContinuousMove",
            },
            CommandPayload::Stop => MappedCommand {
                inner: ptz_worker::Command::Stop,
                label: "Stop",
            },
            CommandPayload::Center { speed } => MappedCommand {
                inner: ptz_worker::Command::Center { speed },
                label: "Center",
            },
        }
    }
}

fn build_loc_header(packet: &EncodedPacket) -> LocHeader {
    let mut extensions = Vec::with_capacity(2);
    extensions.push(LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
        micros_since_unix_epoch: packet.ingest_wallclock_micros,
    }));
    if packet.is_keyframe {
        if let Some(description) = packet.description_base64.as_deref() {
            match general_purpose::STANDARD.decode(description) {
                Ok(bytes) => {
                    extensions.push(LocHeaderExtension::VideoConfig(VideoConfig { data: bytes }));
                }
                Err(err) => {
                    log::warn!("failed to decode avcC base64: {err}");
                }
            }
        }
    }
    LocHeader { extensions }
}

#[derive(Default)]
struct VideoStreamState {
    group_id: u64,
    object_id: u64,
    started: bool,
    stream: Option<SubgroupObjectSender<WEBTRANSPORT>>,
    last_timestamp_us: Option<u64>,
}

struct PendingGroupClose {
    track_alias: u64,
    group_id: u64,
    end_object_id: u64,
    stream: SubgroupObjectSender<WEBTRANSPORT>,
}

impl VideoStreamState {
    fn take_pending_group_close(&mut self, track_alias: u64) -> Option<PendingGroupClose> {
        let stream = self.stream.take()?;
        Some(PendingGroupClose {
            track_alias,
            group_id: self.group_id,
            end_object_id: self.object_id,
            stream,
        })
    }

    async fn start_group(
        &mut self,
        publisher: &moqt::Publisher<WEBTRANSPORT>,
        publication: &PublishedResource,
        publisher_priority: u8,
    ) -> Result<Option<PendingGroupClose>> {
        let mut pending_close = None;
        if self.started {
            pending_close = self.take_pending_group_close(publication.track_alias);
            self.group_id += 1;
        }
        self.object_id = 0;
        let uninit_stream = publisher
            .create_stream(publication)
            .next()
            .await
            .context("open video subgroup stream")?;
        let header = uninit_stream.create_header(
            self.group_id,
            SubgroupId::None,
            publisher_priority,
            false,
            true,
        );
        let stream = uninit_stream.send_header(header).await?;
        self.stream = Some(stream);
        self.started = true;
        Ok(pending_close)
    }
}

fn spawn_pending_group_close(pending: PendingGroupClose) {
    tokio::spawn(async move {
        let PendingGroupClose {
            track_alias,
            group_id,
            end_object_id,
            mut stream,
        } = pending;
        let close_result = async {
            let object = stream.create_object_field(
                0,
                empty_extension_headers(),
                SubgroupObject::new_status(moqt::wire::ObjectStatus::EndOfGroup as u64),
            );
            stream.send(object).await.context("write end of group")?;
            stream.close().await.context("finish subgroup stream")?;
            Result::<()>::Ok(())
        }
        .await;
        match close_result {
            Ok(()) => {
                log::info!(
                    "MoQ close g={} end_o={} alias={}",
                    group_id,
                    end_object_id,
                    track_alias
                );
            }
            Err(err) => {
                log::warn!(
                    "MoQ close failed g={} end_o={} alias={} err={}",
                    group_id,
                    end_object_id,
                    track_alias,
                    err
                );
            }
        }
    });
}

#[derive(Clone)]
struct ProfileTrack {
    track_name: String,
    profile_token: String,
    rtsp_url: String,
    name: Option<String>,
    resolution: Option<(u32, u32)>,
    framerate: Option<f64>,
}

async fn fetch_profile_tracks(
    target: &app_config::Target,
    video_track_prefix: &str,
) -> Result<Vec<ProfileTrack>> {
    let client = soap_client::build(target)?;
    let onvif = onvif_client::OnvifClient::initialize_media(client, target.clone()).await?;
    let profiles = onvif_profile_list::fetch(&onvif).await?;
    let mut tracks = Vec::new();
    for (index, profile) in profiles.into_iter().enumerate() {
        let track_name = format!("{}/profile_{}", video_track_prefix, index + 1);
        let uri = match onvif_stream_uri::fetch(&onvif, &profile.token).await {
            Ok(uri) => uri,
            Err(err) => {
                log::warn!(
                    "GetStreamUri failed for profile_token={}: {err}",
                    profile.token
                );
                continue;
            }
        };
        let rtsp_url = apply_rtsp_credentials(&uri, target);
        let resolution = profile.video_resolution;
        let framerate = profile.video_framerate;
        let resolution_label = resolution
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "-".to_string());
        let framerate_label = framerate
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        let name_label = profile.name.clone().unwrap_or_else(|| "-".to_string());
        log::info!(
            "MoQ video track mapped: track={} profile_token={} name={} resolution={} framerate={} rtsp={}",
            track_name,
            profile.token,
            name_label,
            resolution_label,
            framerate_label,
            redact_rtsp_url(&rtsp_url)
        );
        tracks.push(ProfileTrack {
            track_name,
            profile_token: profile.token,
            rtsp_url,
            name: profile.name,
            resolution,
            framerate,
        });
    }
    if tracks.is_empty() {
        return Err(anyhow!("no usable video profiles"));
    }
    Ok(tracks)
}

fn build_expected_tracks(catalog_track: &str, profile_tracks: &[ProfileTrack]) -> HashSet<String> {
    let mut tracks: HashSet<String> = profile_tracks
        .iter()
        .map(|track| track.track_name.clone())
        .collect();
    tracks.insert(catalog_track.to_string());
    tracks
}

async fn wait_for_video_selection(
    context: VideoSelectionContext<'_>,
    catalog_state: &mut CatalogUpdateState,
) -> Result<(ProfileTrack, PublishedResource)> {
    let VideoSelectionContext {
        session,
        publisher,
        namespace,
        catalog_track,
        expected_tracks,
        profile_tracks,
        publisher_priority,
    } = context;
    let expected_namespace = namespace.join("/");
    loop {
        let event = session.receive_event().await?;
        let SessionEvent::Subscribe(handler) = event else {
            continue;
        };
        let track_name = handler.track_name.clone();
        if handler.track_namespace != expected_namespace || !expected_tracks.contains(&track_name) {
            log::warn!(
                "Subscribe received for unsupported track: ns={} track={}",
                handler.track_namespace,
                track_name
            );
            let _ = handler.error(404, "unsupported track".to_string()).await;
            continue;
        }
        let alias = handler.ok(1_000_000, ContentExists::False).await?;
        let publication = handler.into_publication(alias);
        if track_name == catalog_track {
            catalog_state.track_publication = Some(publication.clone());
            send_catalog(
                publisher,
                &publication,
                catalog_state.next_group_id,
                publisher_priority,
                namespace,
                None,
                profile_tracks,
            )
            .await?;
            catalog_state.next_group_id += 1;
            continue;
        }
        if let Some(profile) = profile_tracks
            .iter()
            .find(|track| track.track_name == track_name)
        {
            log::info!(
                "Selected video track: {} (profile_token={})",
                profile.track_name,
                profile.profile_token
            );
            return Ok((profile.clone(), publication));
        }
        log::warn!("Subscribe received for unknown track: {}", track_name);
    }
}

struct VideoSelectionContext<'a> {
    session: &'a Session<WEBTRANSPORT>,
    publisher: &'a moqt::Publisher<WEBTRANSPORT>,
    namespace: &'a [String],
    catalog_track: &'a str,
    expected_tracks: &'a HashSet<String>,
    profile_tracks: &'a [ProfileTrack],
    publisher_priority: u8,
}

struct CatalogUpdateState {
    track_publication: Option<PublishedResource>,
    next_group_id: u64,
    last_codec: Option<String>,
    selected_track: Option<String>,
}

impl CatalogUpdateState {
    fn new() -> Self {
        Self {
            track_publication: None,
            next_group_id: 0,
            last_codec: None,
            selected_track: None,
        }
    }
}

struct CatalogCodecUpdate<'a> {
    track_name: &'a str,
    codec: &'a str,
}

async fn maybe_send_catalog_update(
    publisher: &moqt::Publisher<WEBTRANSPORT>,
    state: &mut CatalogUpdateState,
    namespace: &[String],
    profiles: &[ProfileTrack],
    publisher_priority: u8,
    packet: &EncodedPacket,
) -> Result<()> {
    let Some(publication) = state.track_publication.as_ref() else {
        return Ok(());
    };
    let Some(track_name) = state.selected_track.as_deref() else {
        return Ok(());
    };
    if !packet.is_keyframe {
        return Ok(());
    }
    if packet.description_base64.is_none() {
        return Ok(());
    }
    let Some(codec) = packet.codec.as_deref() else {
        return Ok(());
    };
    if state.last_codec.as_deref() == Some(codec) {
        return Ok(());
    }
    let update = CatalogCodecUpdate { track_name, codec };
    send_catalog(
        publisher,
        publication,
        state.next_group_id,
        publisher_priority,
        namespace,
        Some(&update),
        profiles,
    )
    .await?;
    state.last_codec = Some(codec.to_string());
    state.next_group_id += 1;
    Ok(())
}

fn build_catalog_tracks(
    namespace: &[String],
    codec_update: Option<&CatalogCodecUpdate<'_>>,
    profiles: &[ProfileTrack],
) -> Vec<Track> {
    let namespace_label = if namespace.is_empty() {
        None
    } else {
        Some(namespace.join("/"))
    };
    profiles
        .iter()
        .map(|profile| {
            let codec = codec_update.and_then(|update| {
                if update.track_name == profile.track_name {
                    Some(update.codec.to_string())
                } else {
                    None
                }
            });
            Track {
                namespace: namespace_label.clone(),
                name: profile.track_name.clone(),
                packaging: Packaging::Known(KnownPackaging::Loc),
                event_type: None,
                role: Some(TrackRole::Known(KnownTrackRole::Video)),
                is_live: true,
                target_latency: None,
                label: profile
                    .name
                    .clone()
                    .or_else(|| Some(profile.profile_token.clone())),
                render_group: None,
                alt_group: None,
                init_data: None,
                depends: None,
                temporal_id: None,
                spatial_id: None,
                codec,
                mime_type: None,
                framerate: profile.framerate,
                timescale: None,
                bitrate: None,
                width: profile.resolution.map(|(width, _)| width),
                height: profile.resolution.map(|(_, height)| height),
                sample_rate: None,
                channel_config: None,
                display_width: None,
                display_height: None,
                lang: None,
                parent_name: None,
                track_duration: None,
            }
        })
        .collect()
}

fn apply_rtsp_credentials(uri: &str, target: &app_config::Target) -> String {
    let (username, password) = target.credentials();
    for prefix in ["rtsp://", "rtsps://"] {
        if let Some(rest) = uri.strip_prefix(prefix) {
            if rest.contains('@') {
                return uri.to_string();
            }
            return format!("{prefix}{username}:{password}@{rest}");
        }
    }
    uri.to_string()
}

fn redact_rtsp_url(uri: &str) -> String {
    for prefix in ["rtsp://", "rtsps://"] {
        if let Some(rest) = uri.strip_prefix(prefix) {
            let Some((creds, host)) = rest.split_once('@') else {
                return uri.to_string();
            };
            let Some((user, _)) = creds.split_once(':') else {
                return uri.to_string();
            };
            return format!("{prefix}{user}:***@{host}");
        }
    }
    uri.to_string()
}

async fn connect_session(
    url: &str,
    insecure_skip_tls_verify: bool,
) -> Result<std::sync::Arc<Session<WEBTRANSPORT>>> {
    let parsed = Url::parse(url).context("parse moqt url")?;
    if parsed.scheme() != "https" {
        bail!("moqt-onvif-client currently supports https:// WebTransport URLs only");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("missing host in moqt url"))?;
    let port = parsed.port().unwrap_or(443);
    let remote_address = (host, port)
        .to_socket_addrs()
        .context("resolve moqt address")?
        .next()
        .ok_or_else(|| anyhow!("failed to resolve moqt address"))?;
    let endpoint = Endpoint::<WEBTRANSPORT>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: !insecure_skip_tls_verify,
    })?;
    let connecting = endpoint
        .connect(remote_address, host)
        .await
        .context("connect moqt transport")?;
    Ok(std::sync::Arc::new(
        connecting.await.context("establish moqt session")?,
    ))
}

fn spawn_command_receiver(
    mut subscriber: moqt::Subscriber<WEBTRANSPORT>,
    subscription: moqt::Subscription,
    command_sender: std_mpsc::Sender<ptz_worker::Command>,
) {
    tokio::spawn(async move {
        let receiver = match subscriber.accept_data_receiver(&subscription).await {
            Ok(receiver) => receiver,
            Err(err) => {
                log::warn!("command accept_data_receiver failed: {err}");
                return;
            }
        };
        let DataReceiver::Datagram(mut datagram) = receiver else {
            log::warn!("command track did not produce datagram receiver");
            return;
        };
        loop {
            let object = match datagram.receive().await {
                Ok(object) => object,
                Err(err) => {
                    log::warn!("command datagram receive failed: {err}");
                    return;
                }
            };
            match object.field.payload() {
                ObjectDatagramPayload::Payload(payload) => {
                    if let Err(err) = handle_command_payload(payload.as_ref(), &command_sender) {
                        log::warn!("command payload error: {err}");
                    }
                }
                ObjectDatagramPayload::Status(_) => {}
            }
        }
    });
}

fn loc_header_to_extension_headers(header: &LocHeader) -> Result<ExtensionHeaders> {
    let mut immutable_extensions = Vec::with_capacity(1);
    let mut encoded = Vec::from(LOC_HEADER_SENTINEL);
    encoded.extend_from_slice(&serde_json::to_vec(header)?);
    immutable_extensions.push(Bytes::from(encoded));

    Ok(ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions,
    })
}

fn empty_extension_headers() -> ExtensionHeaders {
    ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions: vec![],
    }
}

fn parse_namespace(value: &str) -> Vec<String> {
    value
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .collect()
}

fn now_millis() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis() as u64
}

fn now_micros() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_micros() as u64
}

fn init_logger() {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "info");
    env_logger::Builder::from_env(env)
        .format(|buf, record| {
            writeln!(
                buf,
                "{} {:<5} {}",
                buf.timestamp_millis(),
                record.level(),
                record.args()
            )
        })
        .init();
}

fn spawn_rtsp_error_logger(err_rx: std_mpsc::Receiver<String>) {
    std::thread::spawn(move || {
        for err in err_rx {
            log::warn!("RTSP error: {err}");
        }
    });
}

fn spawn_ptz_error_logger(controller: ptz_worker::Controller) {
    tokio::spawn(async move {
        loop {
            if let Some(err) = controller.try_recv_error() {
                log::warn!("PTZ error: {err}");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

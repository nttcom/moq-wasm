use anyhow::{anyhow, bail, Context, Result};
use broadcast::{Broadcast, BroadcastWriter, MoqtTarget, RelayConnection, SourceTracks};
use clap::Parser;
use mediapack::MediaEvent;
use moqt::ClientConfig;
use onvif_ingest::{
    app_config,
    arrival_clamp::ArrivalClamp,
    cli,
    command_subscriber::{CommandSubscriber, CommandTrack},
    onvif_client, onvif_profile_list, onvif_stream_uri, ptz_worker, rtsp_source, soap_client,
};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

const RTSP_EVENT_CAPACITY: usize = 64;
const RTSP_RETRY_MIN_DELAY: Duration = Duration::from_secs(1);
const RTSP_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(author, version, about = "Bridge ONVIF PTZ and RTSP media over MoQ")]
struct MoqtArgs {
    #[command(flatten)]
    onvif: cli::Args,

    /// MoQ relay URL (e.g. https://localhost:4433 for WebTransport, moqt://localhost:4433 for QUIC)
    #[arg(long)]
    moqt_url: String,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long, default_value_t = false)]
    insecure_skip_tls_verify: bool,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Track namespace for publishing video (slash-separated)
    #[arg(long, default_value = "anon/onvif/client")]
    publish_namespace: String,

    /// Track namespace for subscribing to commands (slash-separated)
    #[arg(long, default_value = "anon/onvif/viewer")]
    subscribe_namespace: String,

    /// Track name prefix for video streams
    #[arg(long, default_value = "video")]
    video_track: String,

    /// Track name prefix for audio streams
    #[arg(long, default_value = "audio")]
    audio_track: String,

    /// Track name for ONVIF commands
    #[arg(long, default_value = "command")]
    command_track: String,

    /// Subscriber priority for command track
    #[arg(long, default_value_t = 0)]
    subscriber_priority: u8,

    /// Publish this RTSP stream as profile 1 instead of the camera's ONVIF
    /// profiles, e.g. a local test server
    #[arg(long, value_name = "URL")]
    rtsp_url: Option<String>,

    /// Dump the first keyframe AnnexB payload of profile 1 for ffprobe (default: /tmp/moqt-onvif-keyframe.h264)
    #[arg(
        long,
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "/tmp/moqt-onvif-keyframe.h264"
    )]
    dump_keyframe: Option<PathBuf>,
}

struct ProfileSource {
    tracks: SourceTracks,
    rtsp_url: String,
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

    let command_sender = ptz_worker::spawn(target.clone());
    let sources = match &args.rtsp_url {
        Some(rtsp_url) => vec![ProfileSource {
            tracks: profile_tracks(&args, 1, "RTSP"),
            rtsp_url: rtsp_url.clone(),
        }],
        None => fetch_profile_sources(&target, &args).await?,
    };

    let client_config = ClientConfig {
        port: 0,
        verify_certificate: !args.insecure_skip_tls_verify,
        authorization_token: args.auth_token.clone(),
        follow_go_away: true,
    };
    let mut commands = CommandSubscriber::run(
        &args.moqt_url,
        &client_config,
        CommandTrack {
            namespace: subscribe_namespace.join("/"),
            track: args.command_track.clone(),
            subscriber_priority: args.subscriber_priority,
        },
        command_sender,
    )
    .await?;
    let connection = RelayConnection::new(Some(MoqtTarget {
        url: args.moqt_url.clone(),
        auth_token: args.auth_token.clone(),
        verify_certificate: client_config.verify_certificate,
    }));
    let broadcast = Broadcast::new(connection.clone(), publish_namespace);

    let result = tokio::select! {
        result = publish_profiles(broadcast, sources, args.dump_keyframe) => result,
        result = commands.finished() => result,
    };
    connection.close().await;
    result
}

/// Starts every profile on the first SUBSCRIBE to the catalog or a profile
/// track; fails once the relay has ended every profile.
async fn publish_profiles(
    broadcast: Broadcast,
    sources: Vec<ProfileSource>,
    mut dump_keyframe: Option<PathBuf>,
) -> Result<()> {
    let track_names: Vec<String> = sources
        .iter()
        .flat_map(|source| [source.tracks.video.clone(), source.tracks.audio.clone()])
        .collect();
    broadcast.declare_tracks(&track_names).await?;
    log::info!(
        "Waiting for the first subscriber of {}",
        broadcast.namespace().join("/")
    );
    broadcast.wait_for_subscriber().await?;

    let mut profiles = JoinSet::new();
    for source in sources {
        profiles.spawn(publish_profile(
            broadcast.clone(),
            source,
            dump_keyframe.take(),
        ));
    }
    while let Some(result) = profiles.join_next().await {
        if let Err(err) = result.context("profile task").and_then(|ended| ended) {
            log::warn!("Profile ended: {err:#}");
        }
    }
    bail!("every profile has ended")
}

/// Publishes one profile for as long as the relay accepts it, reconnecting to
/// the camera whenever its RTSP stream ends.
async fn publish_profile(
    broadcast: Broadcast,
    source: ProfileSource,
    mut dump_keyframe: Option<PathBuf>,
) -> Result<()> {
    let mut retry_delay = RTSP_RETRY_MIN_DELAY;
    loop {
        log::info!(
            "Starting RTSP for {}: {}",
            source.tracks.video,
            redact_rtsp_url(&source.rtsp_url)
        );
        let started = Instant::now();
        match play_rtsp(&broadcast, &source, &mut dump_keyframe).await? {
            Ok(()) => log::warn!("RTSP for {} ended", source.tracks.video),
            Err(err) => log::warn!("RTSP for {} failed: {err:#}", source.tracks.video),
        }
        if started.elapsed() >= RTSP_RETRY_MAX_DELAY {
            retry_delay = RTSP_RETRY_MIN_DELAY;
        }
        log::info!(
            "Reconnecting RTSP for {} in {:?}",
            source.tracks.video,
            retry_delay
        );
        tokio::time::sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(RTSP_RETRY_MAX_DELAY);
    }
}

/// Plays one RTSP session into a fresh writer, so a reconnected stream gets a
/// new capture origin for its restarted timestamps. The outer error is the
/// relay's and ends the profile; the inner one is the camera's and triggers a
/// reconnect.
async fn play_rtsp(
    broadcast: &Broadcast,
    source: &ProfileSource,
    dump_keyframe: &mut Option<PathBuf>,
) -> Result<Result<()>> {
    let (event_sender, mut event_receiver) = mpsc::channel(RTSP_EVENT_CAPACITY);
    let (rtsp_result_sender, rtsp_result) = oneshot::channel();
    let rtsp_url = source.rtsp_url.clone();
    // A thread rather than spawn_blocking, which the runtime would wait for on
    // exit while GStreamer still plays.
    std::thread::spawn(move || {
        let _ = rtsp_result_sender.send(rtsp_source::run(&rtsp_url, event_sender));
    });
    let mut writer = BroadcastWriter::run(broadcast.clone(), source.tracks.clone());
    let mut clamp = ArrivalClamp::default();
    while let Some(mut event) = event_receiver.recv().await {
        clamp.apply(&mut event, wall_clock_micros());
        if let (Some(path), MediaEvent::Video(sample)) = (&*dump_keyframe, &event) {
            if sample.is_keyframe {
                std::fs::write(path, &sample.data)
                    .with_context(|| format!("write keyframe payload to {}", path.display()))?;
                log::info!(
                    "RTSP keyframe payload dumped for ffprobe: {}",
                    path.display()
                );
                *dump_keyframe = None;
            }
        }
        writer.push(event)?;
    }
    Ok(rtsp_result
        .await
        .unwrap_or_else(|_| Err(anyhow!("RTSP thread exited"))))
}

fn wall_clock_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as i64)
}

fn profile_tracks(args: &MoqtArgs, profile_index: usize, label: &str) -> SourceTracks {
    SourceTracks {
        video: format!("{}/profile_{profile_index}", args.video_track),
        video_label: label.to_string(),
        audio: format!("{}/profile_{profile_index}", args.audio_track),
        audio_label: format!("{label} audio"),
    }
}

async fn fetch_profile_sources(
    target: &app_config::Target,
    args: &MoqtArgs,
) -> Result<Vec<ProfileSource>> {
    let client = soap_client::build(target)?;
    let onvif = onvif_client::OnvifClient::initialize_media(client, target.clone()).await?;
    let profiles = onvif_profile_list::fetch(&onvif).await?;
    let mut sources = Vec::new();
    for (index, profile) in profiles.into_iter().enumerate() {
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
        let label = profile
            .name
            .clone()
            .unwrap_or_else(|| profile.token.clone());
        let tracks = profile_tracks(args, index + 1, &label);
        log::info!(
            "MoQ media tracks mapped: video_track={} audio_track={} profile_token={} name={} rtsp={}",
            tracks.video,
            tracks.audio,
            profile.token,
            label,
            redact_rtsp_url(&rtsp_url)
        );
        sources.push(ProfileSource { tracks, rtsp_url });
    }
    if sources.is_empty() {
        return Err(anyhow!("no usable video profiles"));
    }
    Ok(sources)
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

fn parse_namespace(value: &str) -> Vec<String> {
    value
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .collect()
}

fn init_logger() {
    // `tracing::span` carries a line per span the moqt session opens.
    let env = env_logger::Env::default().filter_or("RUST_LOG", "info,tracing::span=warn");
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

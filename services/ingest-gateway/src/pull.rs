mod arrival_clamp;
mod onvif;
mod rtsp_source;

use self::arrival_clamp::ArrivalClamp;
use crate::RelayArgs;
use anyhow::{Context, Result, anyhow, bail};
use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use clap::ArgGroup;
use mediapack::MediaEvent;
use moqt::{CertificateVerification, ClientConfig, client::Session};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

const RTSP_EVENT_CAPACITY: usize = 64;
const RTSP_RETRY_MIN_DELAY: Duration = Duration::from_secs(1);
const RTSP_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("source")
        .args(["rtsp_url", "onvif_ip"])
        .required(true)
        .multiple(true)
))]
pub(crate) struct Args {
    #[command(flatten)]
    relay: RelayArgs,

    /// RTSP stream to publish as profile 1 in place of the ONVIF camera's profiles; put credentials in the URL
    #[arg(long, value_name = "URL")]
    rtsp_url: Option<String>,

    #[command(flatten)]
    onvif: onvif::OnvifArgs,

    /// Track namespace for publishing media (slash-separated)
    #[arg(long, default_value = "anon/onvif/client")]
    publish_namespace: String,

    /// Track name prefix for video streams
    #[arg(long, default_value = "video")]
    video_track: String,

    /// Track name prefix for audio streams
    #[arg(long, default_value = "audio")]
    audio_track: String,

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

pub(crate) async fn run(args: Args) -> Result<()> {
    let relay = args.relay.moqt_target().context("--moqt-url is required")?;
    let publish_namespace = parse_namespace(&args.publish_namespace);
    if publish_namespace.is_empty() {
        bail!("publish namespace is required");
    }
    let camera = args.onvif.target()?;
    let sources = match (&args.rtsp_url, &camera) {
        (Some(rtsp_url), _) => vec![ProfileSource {
            tracks: profile_tracks(&args, 1, "RTSP"),
            rtsp_url: rtsp_url.clone(),
        }],
        (None, Some(camera)) => camera_sources(&args, camera).await?,
        (None, None) => unreachable!("clap requires --rtsp-url or --onvif-ip"),
    };

    let client_config = ClientConfig {
        certificate_verification: CertificateVerification::native_roots_or_disabled(
            relay.verify_certificate,
        ),
        authorization_token: relay.auth_token.map(Into::into),
        ..ClientConfig::default()
    };
    let session = Session::connect(&relay.url, &client_config)
        .await
        .context("connect to relay")?;
    let mut commands = match camera {
        Some(camera) => Some(onvif::follow_commands(&session, &args.onvif, camera).await?),
        None => None,
    };
    let connection = RelayConnection::with_client_session(&session);
    let broadcast = Broadcast::new(connection.clone(), publish_namespace);

    let result = tokio::select! {
        result = publish_profiles(broadcast, sources, args.dump_keyframe) => result,
        result = commands_finished(&mut commands) => result,
    };
    connection.close().await;
    result
}

async fn commands_finished(commands: &mut Option<onvif::CommandSubscriber>) -> Result<()> {
    match commands {
        Some(commands) => commands.finished().await,
        None => std::future::pending().await,
    }
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
    tracing::info!(
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
            tracing::warn!("Profile ended: {err:#}");
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
        tracing::info!(
            "Starting RTSP for {}: {}",
            source.tracks.video,
            redact_rtsp_url(&source.rtsp_url)
        );
        let started = Instant::now();
        match play_rtsp(&broadcast, &source, &mut dump_keyframe).await? {
            Ok(()) => tracing::warn!("RTSP for {} ended", source.tracks.video),
            Err(err) => tracing::warn!("RTSP for {} failed: {err:#}", source.tracks.video),
        }
        if started.elapsed() >= RTSP_RETRY_MAX_DELAY {
            retry_delay = RTSP_RETRY_MIN_DELAY;
        }
        tracing::info!(
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
        if let (Some(path), MediaEvent::Video(sample)) = (&*dump_keyframe, &event)
            && sample.is_keyframe
        {
            std::fs::write(path, &sample.data)
                .with_context(|| format!("write keyframe payload to {}", path.display()))?;
            tracing::info!(
                "RTSP keyframe payload dumped for ffprobe: {}",
                path.display()
            );
            *dump_keyframe = None;
        }
        writer.push(event)?;
    }
    Ok(rtsp_result
        .await
        .unwrap_or_else(|_| Err(anyhow!("RTSP thread exited"))))
}

fn wall_clock_micros() -> i64 {
    moqt::executor::system_time().as_micros() as i64
}

fn profile_tracks(args: &Args, profile_index: usize, label: &str) -> SourceTracks {
    SourceTracks {
        video: format!("{}/profile_{profile_index}", args.video_track),
        video_label: label.to_string(),
        audio: format!("{}/profile_{profile_index}", args.audio_track),
        audio_label: format!("{label} audio"),
        ..SourceTracks::default()
    }
}

async fn camera_sources(args: &Args, camera: &onvif::Target) -> Result<Vec<ProfileSource>> {
    let profiles = onvif::fetch_profiles(camera).await?;
    Ok(profiles
        .into_iter()
        .enumerate()
        .map(|(index, profile)| {
            let tracks = profile_tracks(args, index + 1, &profile.label);
            tracing::info!(
                "MoQ media tracks mapped: video_track={} audio_track={} profile_token={} name={} rtsp={}",
                tracks.video,
                tracks.audio,
                profile.token,
                profile.label,
                redact_rtsp_url(&profile.rtsp_url)
            );
            ProfileSource {
                tracks,
                rtsp_url: profile.rtsp_url,
            }
        })
        .collect())
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

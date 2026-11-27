mod annexb;

use std::{path::PathBuf, time::Duration};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use moq_wasm::{
    CertificateVerification, ConnectOptions, Retention, Session, SessionEnd,
    media::{
        self, BitratePreference, EventTimelineOptions, FollowOptions, MediaSourceOptions,
        RemoteBroadcast, TrackQuery, WatchOptions,
    },
};
use serde_json::json;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    relay: RelayArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct RelayArgs {
    /// The relay: `moqt://` over raw QUIC, `https://` over WebTransport, `wss://` over WebSocket.
    #[arg(long, default_value = "moqt://127.0.0.1:4433")]
    url: String,
    /// The relay's certificate file, for a self-signed one.
    #[arg(long)]
    cert: Option<PathBuf>,
    /// Accept any certificate.
    #[arg(long, conflicts_with = "cert")]
    insecure: bool,
    /// The JWT of a relay that verifies tokens.
    #[arg(long)]
    token: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Publishes an H.264 Annex B file, looping, as the video of a broadcast.
    Publish {
        /// An H.264 Annex B file without B-frames.
        file: PathBuf,
        /// The broadcast's namespace, `/`-separated.
        #[arg(long, default_value = "anon/live/test")]
        namespace: String,
        /// The frame rate the file is sent at.
        #[arg(long, default_value_t = 30)]
        fps: u32,
    },
    /// Prints each broadcast under a prefix: its catalog, its events and the
    /// frame rate and bitrate of its lowest video.
    Watch {
        /// The namespaces to follow, `/`-separated.
        #[arg(long, default_value = "anon/live")]
        prefix: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "moq_wasm=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let session = connect(&cli.relay).await?;
    let work = async {
        match cli.command {
            Command::Publish {
                file,
                namespace,
                fps,
            } => publish(&session, &file, &namespace, fps).await,
            Command::Watch { prefix } => {
                println!("watching {prefix}");
                let prefix: Vec<&str> = prefix.split('/').collect();
                media::watch_broadcasts(&session, &prefix, WatchOptions::default(), watch_one).await
            }
        }
    };
    tokio::select! {
        result = work => result?,
        end = session.closed() => match end {
            SessionEnd::Closed => {}
            end => anyhow::bail!("the session ended: {end:?}"),
        },
        _ = tokio::signal::ctrl_c() => {}
    }
    session.close().await;
    Ok(())
}

async fn connect(relay: &RelayArgs) -> anyhow::Result<Session> {
    let certificate_verification = match (&relay.cert, relay.insecure) {
        (Some(cert), _) => CertificateVerification::TrustedCertificates(cert.clone()),
        (None, true) => CertificateVerification::Disabled,
        (None, false) => CertificateVerification::default(),
    };
    let options = ConnectOptions {
        token: relay.token.clone(),
        certificate_verification,
        ..ConnectOptions::default()
    };
    moq_wasm::connect(&relay.url, options)
        .await
        .with_context(|| format!("connect to {}", relay.url))
}

async fn publish(
    session: &Session,
    file: &PathBuf,
    namespace: &str,
    fps: u32,
) -> anyhow::Result<()> {
    let stream = tokio::fs::read(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    let codec = annexb::codec_string(&stream).context("the file has no H.264 SPS")?;
    let frames = annexb::access_units(&stream);
    println!(
        "publishing {} frames of {codec} at {fps} fps as {namespace}",
        frames.len()
    );

    // Register tracks before announcing, so a SUBSCRIBE the relay forwards at once finds them.
    let builder = media::create_broadcast(session, &namespace.split('/').collect::<Vec<_>>());
    let events = builder
        .event_timeline("events", Retention::For(Duration::from_secs(3600)))
        .await?;
    let broadcast = builder.announce().await?;
    let camera = broadcast.add_media_source(MediaSourceOptions {
        video_framerate: Some(fps),
        ..MediaSourceOptions::new(file.display().to_string())
    });
    camera.configure_video(&codec)?;

    let frame_interval = Duration::from_secs(1) / fps;
    let pushing = async {
        let mut ticks = tokio::time::interval(frame_interval);
        let mut timestamp_micros = 0;
        for lap in 0u64.. {
            events
                .append(&json!({"T": timestamp_micros / 1000, "data": {"lap": lap}}))
                .await?;
            for frame in &frames {
                ticks.tick().await;
                camera.push_video(frame.to_vec(), timestamp_micros)?;
                timestamp_micros += frame_interval.as_micros() as u64;
            }
        }
        anyhow::Ok(())
    };
    tokio::select! {
        result = pushing => result?,
        _ = tokio::signal::ctrl_c() => {}
    }
    // PUBLISH_NAMESPACE_DONE before the session closes.
    broadcast.close().await
}

async fn watch_one(remote: RemoteBroadcast) -> anyhow::Result<()> {
    let name = remote.namespace().join("/");
    println!("{name}: published");
    let mut catalogs = remote.catalogs().await?;
    let catalog = catalogs.next().await?.context("the catalog track ended")?;
    for track in catalog.tracks() {
        println!(
            "{name}: track {} {:?} {:?} {:?} bps",
            track.name, track.role, track.codec, track.bitrate
        );
    }
    let video = catalog
        .select(&TrackQuery {
            role: Some("video"),
            prefer: Some(BitratePreference::Lowest),
            ..TrackQuery::default()
        })
        .map(|track| track.name.clone())
        .context("the catalog lists no video")?;
    let mut frames = remote.follow(&video, FollowOptions::default());
    let mut events = remote.event_timeline("events", EventTimelineOptions::default());
    let mut report = tokio::time::interval(Duration::from_secs(1));
    let (mut count, mut bytes) = (0, 0);
    loop {
        tokio::select! {
            frame = frames.next() => {
                count += 1;
                bytes += frame.payload.len();
            }
            record = events.next() => println!("{name}: event {record}"),
            _ = report.tick() => {
                println!("{name}: {video} {count} frames, {} kbit/s", bytes * 8 / 1000);
                (count, bytes) = (0, 0);
            }
        }
    }
}

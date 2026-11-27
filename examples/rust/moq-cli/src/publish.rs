use anyhow::{Context, Result};
use moq_wasm::{
    Session,
    media::{self, AnnexBSplitter, MediaSourceOptions, TrackKind},
};
use tokio::{io::AsyncReadExt, time::Instant};
use tracing::info;

use crate::cli::{Codec, PublishArgs};
use crate::track::Namespace;
use crate::transport::{connect_relay, run_until_closed};

const READ_BUFFER_BYTES: usize = 64 * 1024;

pub async fn run(args: PublishArgs) -> Result<()> {
    let session = connect_relay(&args.relay).await?;
    run_until_closed(
        &session,
        publish_stdin(&session, &args.namespace, args.codec),
    )
    .await
}

/// Publishes stdin as the `video` track of an MSF broadcast in `namespace`.
async fn publish_stdin(session: &Session, namespace: &Namespace, codec: Codec) -> Result<()> {
    let broadcast = media::create_broadcast(session, &namespace.0)
        .announce()
        .await
        .context("failed to publish the namespace")?;
    info!(track_namespace = %namespace.0.join("/"), "broadcast started");
    // The stream has keyframes only where the encoder put them, so its
    // tracks are sent and cached at the relay even while nobody subscribes.
    let source = broadcast.add_media_source(MediaSourceOptions {
        on_demand: false,
        ..MediaSourceOptions::new("Video")
    });
    // The catalog lists an `audio` placeholder next to `video` until an audio
    // configuration arrives; this publisher sends video only.
    source
        .remove_track(TrackKind::Audio)
        .await
        .context("failed to remove the audio track")?;
    let codec = codec.codec_family();
    source.configure_video(codec)?;
    let mut splitter = AnnexBSplitter::new(codec)?;
    let mut video_started = false;
    // The stream has no timestamps: each access unit takes its arrival, 1 µs
    // after the previous one when one read completes several.
    let started = Instant::now();
    let mut last_micros = None;
    let mut arrival = || {
        let elapsed = started.elapsed().as_micros() as u64;
        let micros = last_micros.map_or(elapsed, |last: u64| elapsed.max(last + 1));
        last_micros = Some(micros);
        micros
    };

    let mut reader = tokio::io::stdin();
    let mut read_buf = vec![0u8; READ_BUFFER_BYTES];
    // Keeps the listener registered while the loop body pushes frames: tokio
    // drops a SIGINT that arrives while none is registered.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        let read = tokio::select! {
            result = reader.read(&mut read_buf) => result?,
            _ = &mut ctrl_c => {
                info!("received ctrl-c, shutting down");
                break;
            }
        };
        let access_units = if read == 0 {
            splitter.finish()
        } else {
            splitter.push(&read_buf[..read])
        }
        .context("failed to read the input; check --codec")?;
        for access_unit in access_units {
            source.push_video(access_unit, arrival())?;
            if !video_started {
                info!("video started");
                video_started = true;
            }
        }
        if read == 0 {
            info!("input ended");
            break;
        }
    }
    broadcast.close().await
}

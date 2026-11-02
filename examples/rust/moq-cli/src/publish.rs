use std::sync::Arc;

use anyhow::{Context, Result};
use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use mediapack::{Timestamp, h264::ElementaryStreamDemuxer};
use moqt::{RequestErrorCode, Session, SessionEvent, publish::Origin};
use tokio::{io::AsyncReadExt, time::Instant};
use tracing::{debug, info, warn};

use crate::cli::PublishArgs;
use crate::track::Namespace;
use crate::transport::connect_relay;

const READ_BUFFER_BYTES: usize = 64 * 1024;

pub async fn run(args: PublishArgs) -> Result<()> {
    let connection = connect_relay(&args.relay, args.namespace.app_id()).await?;
    let result = publish_stdin(&connection.session, &args.namespace).await;
    connection.close().await;
    result
}

/// Publishes stdin as the `video` track of an MSF broadcast in `namespace`.
async fn publish_stdin(session: &Arc<Session>, namespace: &Namespace) -> Result<()> {
    let relay = RelayConnection::with_session(session.clone(), Origin::default());
    let broadcast = Broadcast::new(relay.clone(), namespace.0.clone());
    broadcast
        .announce()
        .await
        .context("failed to publish the namespace")?;
    info!(track_namespace = %namespace.0.join("/"), "broadcast started");
    let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());
    let mut demuxer = ElementaryStreamDemuxer::new();
    let started = Instant::now();
    let arrival = || Timestamp::from_micros(started.elapsed().as_micros() as u64);

    let mut reader = tokio::io::stdin();
    let mut read_buf = vec![0u8; READ_BUFFER_BYTES];
    // Keeps the listener registered while the loop body writes frames: tokio
    // drops a SIGINT that arrives while none is registered.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        let read = tokio::select! {
            result = reader.read(&mut read_buf) => result?,
            event = session.receive_event() => {
                if let Some(event) = relay.handle(event?).await {
                    answer_unhandled(event).await;
                }
                continue;
            }
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
            _ = &mut ctrl_c => {
                info!("received ctrl-c, shutting down");
                return Ok(());
            }
        };
        let events = if read == 0 {
            demuxer.finish(arrival())?
        } else {
            demuxer.push(&read_buf[..read], arrival())?
        };
        for event in events {
            writer.push(event)?;
        }
        if read == 0 {
            info!("input ended");
            return Ok(());
        }
    }
}

/// The broadcast answers the requests for its own tracks; a SUBSCRIBE for
/// any other track is rejected so the relay does not wait for an answer.
async fn answer_unhandled(event: SessionEvent) {
    match event {
        SessionEvent::Subscribe(handler) => {
            let reason = format!(
                "{}/{} is not published here",
                handler.track_namespace, handler.track_name
            );
            if let Err(error) = handler
                .error(RequestErrorCode::TrackDoesNotExist, reason)
                .await
            {
                warn!(%error, "failed to reject SUBSCRIBE");
            }
        }
        event => debug!(?event, "session event ignored"),
    }
}

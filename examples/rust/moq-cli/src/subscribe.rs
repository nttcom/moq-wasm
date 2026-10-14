use std::{pin::Pin, time::Duration};

use anyhow::{Context, Result};
use moqt::{SessionEvent, TrackReader};
use tokio::{io::AsyncWriteExt, task::JoinHandle, time::Sleep};
use tracing::info;

use crate::catalog;
use crate::cli::{RelayArgs, SubscribeArgs};
use crate::track::FullTrackName;
use crate::transport::{
    MOVE_TIMEOUT, RelayConnection, connect_relay, follow_go_away, read_catalog, subscribe,
    track_reader,
};

const MOVE_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// How long the previous relay session stays open after a move for groups it
/// started and has not finished.
const PREVIOUS_SESSION_GRACE: Duration = Duration::from_secs(5);

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let mut relay = args.relay.clone();
    let mut watching = watch(&relay, &args.track).await?;
    // The move runs beside the current session, which keeps delivering
    // until the new relay has accepted the subscriptions (draft-14 §8.3.1).
    let mut pending_move: Option<JoinHandle<Result<Watching>>> = None;
    // After a move the previous session finishes the groups before the
    // cutover (the first group the new session delivers, or the one after
    // the newest group written), and the new session supplies the rest.
    let mut previous: Option<Watching> = None;
    let mut cutover: Option<u64> = None;
    let mut grace: Option<Pin<Box<Sleep>>> = None;
    let mut newest_written: Option<u64> = None;
    let mut out = tokio::io::stdout();
    loop {
        let session = watching.connection.session.clone();
        tokio::select! {
            object = watching.media_reader.next_object() => match object {
                Ok(Some(object)) => {
                    if previous.is_some() {
                        let first = *cutover.get_or_insert(match newest_written {
                            Some(newest) if object.group_id <= newest => newest + 1,
                            _ => object.group_id,
                        });
                        if object.group_id < first {
                            continue;
                        }
                    }
                    newest_written = newest_written.max(Some(object.group_id));
                    out.write_all(&object.payload).await?;
                    out.flush().await?;
                }
                Ok(None) => break,
                Err(error) => tracing::warn!(%error, "group read error, advancing to next group"),
            },
            object = async { previous.as_mut().expect("polled only while the previous session is open").media_reader.next_object().await },
                if previous.is_some() =>
            {
                match object {
                    Ok(Some(object)) if cutover.is_none_or(|first| object.group_id < first) => {
                        newest_written = newest_written.max(Some(object.group_id));
                        out.write_all(&object.payload).await?;
                        out.flush().await?;
                    }
                    // Reached the cutover, or the previous relay ended the track.
                    _ => {
                        grace = None;
                        if let Some(previous) = previous.take() {
                            previous.close().await;
                            info!("closed the previous relay session");
                        }
                    }
                }
            },
            () = async { grace.as_mut().expect("polled only while the grace runs").await },
                if grace.is_some() =>
            {
                grace = None;
                if let Some(previous) = previous.take() {
                    previous.close().await;
                    info!("closed the previous relay session after the grace period");
                }
            },
            event = session.receive_event() => {
                if let SessionEvent::GoAway(go_away) = event?
                    && pending_move.is_none()
                {
                    follow_go_away(&mut relay, go_away.new_session_uri())?;
                    info!(relay = %relay.url, "GOAWAY received; subscribing on the relay it names");
                    let (relay, track) = (relay.clone(), args.track.clone());
                    pending_move = Some(tokio::spawn(async move {
                        tokio::time::timeout(MOVE_TIMEOUT, watch_until_available(&relay, &track))
                            .await
                            .context("timed out subscribing on the relay named by GOAWAY")?
                    }));
                }
            },
            moved = async { pending_move.as_mut().expect("polled only while a move is pending").await },
                if pending_move.is_some() =>
            {
                pending_move = None;
                match moved? {
                    Ok(next) => {
                        if let Some(older) = previous.take() {
                            older.close().await;
                        }
                        previous = Some(std::mem::replace(&mut watching, next));
                        cutover = None;
                        grace = Some(Box::pin(tokio::time::sleep(PREVIOUS_SESSION_GRACE)));
                        info!(relay = %relay.url, "moved to the relay named by GOAWAY");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to move to the relay named by GOAWAY; staying on the current session");
                    }
                }
            },
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
        }
    }
    if let Some(pending_move) = pending_move {
        pending_move.abort();
    }
    if let Some(previous) = previous {
        previous.close().await;
    }
    watching.close().await;
    Ok(())
}

struct Watching {
    connection: RelayConnection,
    media_reader: TrackReader,
}

impl Watching {
    /// Every other handle to the session must be dropped first.
    async fn close(self) {
        drop(self.media_reader);
        self.connection.close().await;
    }
}

/// The publisher moves at the same time as this subscriber, so the track may
/// not exist on the new relay yet; the SUBSCRIBE is repeated until it does.
async fn watch_until_available(relay: &RelayArgs, track: &FullTrackName) -> Result<Watching> {
    loop {
        match watch(relay, track).await {
            Ok(watching) => return Ok(watching),
            Err(error) => {
                tracing::info!(%error, "not subscribed on the new relay yet; retrying");
                tokio::time::sleep(MOVE_RETRY_INTERVAL).await;
            }
        }
    }
}

async fn watch(relay: &RelayArgs, track: &FullTrackName) -> Result<Watching> {
    let connection = connect_relay(relay, track.app_id()).await?;
    let session = &connection.session;

    // Subscribe to both tracks before reading either so NextGroupStart
    // catches the first media group of a burst publisher.
    let catalog_subscription =
        subscribe(session, &track.namespace, msf::CATALOG_TRACK_NAME).await?;
    info!(
        namespace = track.namespace,
        track = track.name,
        "subscribing"
    );
    let media_subscription = subscribe(session, &track.namespace, &track.name).await?;
    info!("subscribed");

    let catalog = read_catalog(session, &catalog_subscription).await?;
    describe_catalog(&catalog::parse(&catalog)?);
    let media_reader = track_reader(session, &media_subscription).await?;
    Ok(Watching {
        connection,
        media_reader,
    })
}

fn describe_catalog(catalog: &msf::Catalog) {
    let Some(track) = catalog.tracks.as_ref().and_then(|tracks| tracks.first()) else {
        info!("catalog has no tracks");
        return;
    };
    info!(
        track = track.name,
        packaging = crate::catalog::packaging_str(&track.packaging),
        codec = track.codec.as_deref().unwrap_or("-"),
        "catalog resolved"
    );
}

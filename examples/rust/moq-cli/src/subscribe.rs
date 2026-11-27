use anyhow::{Context, Result};
use moq_wasm::{GroupStartOptions, Session, Subscription, media::Catalog};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::cli::SubscribeArgs;
use crate::track::FullTrackName;
use crate::transport::{connect_relay, run_until_closed};

const CATALOG_TRACK_NAME: &str = "catalog";

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let session = connect_relay(&args.relay).await?;
    run_until_closed(&session, watch(&session, &args.track)).await
}

async fn watch(session: &Session, track: &FullTrackName) -> Result<()> {
    let media = subscribe_media(session, track).await?;
    let mut out = tokio::io::stdout();
    // Keeps the listener registered while the loop body writes objects: tokio
    // drops a SIGINT that arrives while none is registered.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            object = media.next() => match object {
                Ok(Some(object)) => {
                    out.write_all(&object.payload).await?;
                    out.flush().await?;
                }
                Ok(None) => return Ok(()),
                Err(error) => return Err(error.context("the track is malformed")),
            },
            _ = &mut ctrl_c => {
                info!("received ctrl-c, shutting down");
                return Ok(());
            }
        }
    }
}

async fn subscribe_media(session: &Session, track: &FullTrackName) -> Result<Subscription> {
    let namespace = &track.namespace.0;
    let track_namespace = namespace.join("/");
    let catalogs = session
        .subscribe_current_group(namespace, CATALOG_TRACK_NAME, GroupStartOptions::default())
        .await
        .with_context(|| format!("failed to subscribe {track_namespace}/{CATALOG_TRACK_NAME}"))?;
    // Starts at the current group, or asks a track with long groups to start
    // one (draft-18 §5.1.3.1).
    let media = session
        .subscribe_group_start(namespace, &track.name, 0, GroupStartOptions::default())
        .await
        .with_context(|| format!("failed to subscribe {track_namespace}/{}", track.name))?;
    info!(track_namespace, track_name = track.name, "subscribed");

    let catalog = catalogs
        .next()
        .await?
        .context("catalog track ended before any object")?;
    let catalog = Catalog::parse(&catalog.payload).context("catalog is not MSF JSON")?;
    describe_track(&catalog, &track.name);
    Ok(media)
}

fn describe_track(catalog: &Catalog, track_name: &str) {
    let Some(track) = catalog
        .tracks()
        .iter()
        .find(|track| track.name == track_name)
    else {
        info!(track_name, "the catalog does not list the track");
        return;
    };
    info!(
        track_name,
        packaging = ?track.packaging,
        codec = track.codec.as_deref().unwrap_or("-"),
        "catalog resolved"
    );
}

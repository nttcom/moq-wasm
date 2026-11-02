use anyhow::{Context, Result};
use moqt::{
    Session, TrackReader,
    subscribe::{FilterType, SubscribeOption},
};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::cli::SubscribeArgs;
use crate::track::FullTrackName;
use crate::transport::connect_relay;

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let connection = connect_relay(&args.relay, args.track.app_id()).await?;
    let result = watch(&connection.session, &args.track).await;
    connection.close().await;
    result
}

async fn watch(session: &Session, track: &FullTrackName) -> Result<()> {
    let mut media_reader = subscribe_media(session, track).await?;
    let mut out = tokio::io::stdout();
    // Keeps the listener registered while the loop body writes objects: tokio
    // drops a SIGINT that arrives while none is registered.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            object = media_reader.next_object() => match object {
                Ok(Some(object)) => {
                    out.write_all(&object.payload).await?;
                    out.flush().await?;
                }
                Ok(None) => return Ok(()),
                Err(error) => return Err(error.context("the track is malformed")),
            },
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
            _ = &mut ctrl_c => {
                info!("received ctrl-c, shutting down");
                return Ok(());
            }
        }
    }
}

async fn subscribe_media(session: &Session, track: &FullTrackName) -> Result<TrackReader> {
    // Subscribe to both tracks before reading either so NextGroupStart
    // catches the first media group of a burst publisher.
    let mut subscriber = session.subscriber();
    let (_catalog_subscription, mut catalog_reader) = subscriber
        .subscribe_current_group(track.namespace.clone(), msf::CATALOG_TRACK_NAME.to_string())
        .await
        .with_context(|| {
            format!(
                "failed to subscribe {}/{}",
                track.namespace,
                msf::CATALOG_TRACK_NAME
            )
        })?;
    let option = SubscribeOption {
        filter_type: FilterType::NextGroupStart,
        ..SubscribeOption::default()
    };
    let media_subscription = subscriber
        .subscribe(track.namespace.clone(), track.name.clone(), option)
        .await
        .with_context(|| format!("failed to subscribe {}/{}", track.namespace, track.name))?;
    info!(
        track_namespace = track.namespace,
        track_name = track.name,
        "subscribed"
    );

    let catalog = catalog_reader
        .next_object()
        .await?
        .context("catalog track ended before any object")?;
    let catalog: msf::Catalog =
        serde_json::from_slice(&catalog.payload).context("catalog is not MSF JSON")?;
    describe_track(&catalog, &track.name);
    Ok(subscriber.track_reader(&media_subscription))
}

fn describe_track(catalog: &msf::Catalog, track_name: &str) {
    let Some(track) = catalog
        .tracks
        .iter()
        .flatten()
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

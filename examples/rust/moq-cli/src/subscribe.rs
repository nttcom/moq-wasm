use anyhow::Result;
use moqt::{Session, TrackReader, subscribe::FilterType};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::catalog;
use crate::cli::SubscribeArgs;
use crate::track::FullTrackName;
use crate::transport::{connect_relay, read_catalog, subscribe, track_reader};

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let connection = connect_relay(&args.relay, args.track.app_id()).await?;
    // `watch` drops every handle to the session on return, errors included,
    // so `close` can finish.
    let result = watch(&connection.session, &args.track).await;
    connection.close().await;
    result
}

async fn watch(session: &Session, track: &FullTrackName) -> Result<()> {
    let mut media_reader = subscribe_media(session, track).await?;
    let mut out = tokio::io::stdout();
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
        }
    }
}

async fn subscribe_media(session: &Session, track: &FullTrackName) -> Result<TrackReader> {
    // Subscribe to both tracks before reading either so NextGroupStart
    // catches the first media group of a burst publisher. The catalog's
    // joining FETCH may only join a Largest Object subscription (draft-14
    // §9.16.2).
    let catalog_subscription = subscribe(
        session,
        &track.namespace,
        msf::CATALOG_TRACK_NAME,
        FilterType::LargestObject,
    )
    .await?;
    info!(
        namespace = track.namespace,
        track = track.name,
        "subscribing"
    );
    let media_subscription = subscribe(
        session,
        &track.namespace,
        &track.name,
        FilterType::NextGroupStart,
    )
    .await?;
    info!("subscribed");

    let catalog = read_catalog(session, &catalog_subscription).await?;
    describe_catalog(&catalog::parse(&catalog)?);
    track_reader(session, &media_subscription).await
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

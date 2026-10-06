use anyhow::Result;
use moqt::{Session, TrackReader};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::catalog;
use crate::cli::SubscribeArgs;
use crate::transport::{connect_relay, session_closed, subscribe_track};

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let connection = connect_relay(&args.relay, args.track.app_id()).await?;
    let session_watch = tokio::spawn({
        let session = connection.session.clone();
        async move {
            let reason = session_closed(&session).await;
            tracing::error!(%reason, "session ended");
        }
    });
    let result = subscribe_stdout(&connection.session, &args).await;
    session_watch.abort();
    connection.close().await;
    result
}

async fn subscribe_stdout(session: &Session, args: &SubscribeArgs) -> Result<()> {
    let track = &args.track;

    // Subscribe to both tracks up front so NextGroupStart catches the first
    // group; otherwise a burst publisher can finish before we are done
    // reading the catalog and the media subscription would start too late.
    let mut catalog_reader =
        TrackReader::new(subscribe_track(session, &track.namespace, catalog::TRACK).await?);
    info!(
        namespace = track.namespace,
        track = track.name,
        "subscribing"
    );
    let mut media_reader =
        TrackReader::new(subscribe_track(session, &track.namespace, &track.name).await?);
    info!("subscribed");

    match catalog_reader.next_object().await? {
        Some(object) => describe_catalog(&catalog::parse(&object.payload)?),
        None => anyhow::bail!("catalog stream ended before any object"),
    }

    let mut out = tokio::io::stdout();
    loop {
        match media_reader.next_object().await {
            Ok(Some(object)) => {
                out.write_all(&object.payload).await?;
                out.flush().await?;
            }
            Ok(None) => break,
            Err(error) => tracing::warn!(%error, "group read error, advancing to next group"),
        }
    }

    Ok(())
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

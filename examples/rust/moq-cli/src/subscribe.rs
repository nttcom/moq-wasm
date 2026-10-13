use anyhow::Result;
use moqt::Session;
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::catalog;
use crate::cli::SubscribeArgs;
use crate::transport::{connect_relay, read_catalog, subscribe, track_reader};

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let connection = connect_relay(&args.relay, args.track.app_id()).await?;
    let session_watch = tokio::spawn({
        let session = connection.session.clone();
        async move {
            let close = session.closed().await;
            tracing::error!(%close, "session ended");
        }
    });
    let result = subscribe_stdout(&connection.session, &args).await;
    session_watch.abort();
    connection.close().await;
    result
}

async fn subscribe_stdout(session: &Session, args: &SubscribeArgs) -> Result<()> {
    let track = &args.track;

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
    let mut media_reader = track_reader(session, &media_subscription).await?;

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

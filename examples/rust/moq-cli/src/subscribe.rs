use std::sync::Arc;

use anyhow::Result;
use moqt::SessionEvent;
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::catalog;
use crate::cli::SubscribeArgs;
use crate::transport::{TrackReader, connect_session, subscribe_track};

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let track = &args.track;
    let session = Arc::new(connect_session(&args.relay, args.insecure).await?);

    tokio::spawn({
        let session = session.clone();
        async move {
            loop {
                match session.receive_event().await {
                    Ok(SessionEvent::ProtocolViolation()) => {
                        tracing::error!("protocol violation");
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!("event loop error: {e}");
                        break;
                    }
                }
            }
        }
    });

    // Subscribe to both tracks up front so NextGroupStart catches the first
    // group; otherwise a burst publisher can finish before we are done
    // reading the catalog and the media subscription would start too late.
    let mut catalog_reader =
        TrackReader::new(subscribe_track(&session, &track.namespace, catalog::TRACK).await?);
    info!(
        namespace = track.namespace,
        track = track.name,
        "subscribing"
    );
    let mut media_reader =
        TrackReader::new(subscribe_track(&session, &track.namespace, &track.name).await?);
    info!("subscribed");

    match catalog_reader.next_object().await? {
        Some(object) => describe_catalog(&catalog::parse(&object)?),
        None => anyhow::bail!("catalog stream ended before any object"),
    }

    let mut out = tokio::io::stdout();
    while let Some(object) = media_reader.next_object().await? {
        out.write_all(&object).await?;
        out.flush().await?;
    }

    Ok(())
}

fn describe_catalog(catalog: &media_streaming_format::Catalog) {
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

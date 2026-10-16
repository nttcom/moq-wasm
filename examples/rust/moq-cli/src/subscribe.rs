use anyhow::Result;
use moqt::{TrackReader, subscribe::FilterType};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::catalog;
use crate::cli::SubscribeArgs;
use crate::transport::{RelayConnection, connect_relay, read_catalog, subscribe, track_reader};

pub async fn run(args: SubscribeArgs) -> Result<()> {
    let mut watching = watch(&args).await?;
    let session = watching.connection.session.clone();
    let mut out = tokio::io::stdout();
    loop {
        tokio::select! {
            object = watching.media_reader.next_object() => match object {
                Ok(Some(object)) => {
                    out.write_all(&object.payload).await?;
                    out.flush().await?;
                }
                Ok(None) => break,
                Err(error) => tracing::warn!(%error, "group read error, advancing to next group"),
            },
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
        }
    }
    drop(session);
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

async fn watch(args: &SubscribeArgs) -> Result<Watching> {
    let track = &args.track;
    let connection = connect_relay(&args.relay, track.app_id()).await?;
    let session = &connection.session;

    // Subscribe to both tracks before reading either so NextGroupStart
    // catches the first media group of a burst publisher.
    let catalog_subscription = subscribe(
        session,
        &track.namespace,
        msf::CATALOG_TRACK_NAME,
        FilterType::NextGroupStart,
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

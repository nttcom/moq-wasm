use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::PublishOption;
use tokio::io::AsyncReadExt;
use tracing::info;

use crate::catalog;
use crate::cli::{Container, PublishArgs};
use crate::loc;
use crate::media::{Frame, h264::AnnexBFramer};
use crate::transport::{TrackWriter, connect_session};

const READ_BUFFER_BYTES: usize = 64 * 1024;
const DRAIN_BEFORE_CLOSE: std::time::Duration = std::time::Duration::from_millis(500);

/// Wall-clock microseconds since the Unix epoch.
fn unix_micros_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as i64)
        .unwrap_or(0)
}

pub async fn run(args: PublishArgs) -> Result<()> {
    if args.container == Container::Cmaf {
        anyhow::bail!("--container cmaf is not implemented yet");
    }

    let track = &args.track;
    let session = connect_session(&args.relay, args.insecure).await?;

    let publisher = session.publisher();
    let subscription = publisher
        .publish(
            track.namespace.clone(),
            track.name.clone(),
            PublishOption::default(),
        )
        .await
        .context("failed to publish")?;
    let catalog_subscription = publisher
        .publish(
            track.namespace.clone(),
            catalog::TRACK.to_string(),
            PublishOption::default(),
        )
        .await
        .context("failed to publish catalog track")?;
    info!(
        namespace = track.namespace,
        track = track.name,
        "publish ok"
    );

    let mut media = TrackWriter::new(publisher.create_stream(&subscription));
    let mut catalog = CatalogPublisher::new(
        TrackWriter::new(publisher.create_stream(&catalog_subscription)),
        track.namespace.clone(),
        track.name.clone(),
    );
    let mut framer = AnnexBFramer::new(args.codec.clone());

    let mut reader = tokio::io::stdin();
    let mut read_buf = vec![0u8; READ_BUFFER_BYTES];
    let mut frame_index: u64 = 0;
    // Wall-clock time of the frame timeline's zero point, set on the first frame.
    // Maps each frame's relative PTS onto a Unix-epoch capture timestamp.
    let mut capture_origin: Option<i64> = None;

    loop {
        let read = tokio::select! {
            result = reader.read(&mut read_buf) => result?,
            _ = tokio::signal::ctrl_c() => {
                info!("received ctrl-c, shutting down");
                break;
            }
        };
        let frames = if read == 0 {
            framer.flush()?
        } else {
            framer.feed(&read_buf[..read])?
        };

        for frame in frames {
            let codec = framer
                .codec_string()
                .context("first frame is not a keyframe (no SPS yet)")?;
            catalog.maybe_publish(&frame, &codec).await?;
            if frame.keyframe {
                media.start_group().await?;
            }
            let origin =
                *capture_origin.get_or_insert_with(|| unix_micros_now() - frame.timestamp.0);
            let capture = loc::capture_timestamp_ext((origin + frame.timestamp.0).max(0) as u64);
            media.write(frame.payload, vec![capture]).await?;
            frame_index += 1;
        }

        if read == 0 {
            break;
        }
    }

    let groups = media.groups();
    media.finish().await?;
    catalog.finish().await?;
    info!(frames = frame_index, groups, "publish done");

    // Let the relay drain in-flight stream data before the QUIC session is
    // dropped on return; otherwise trailing objects are truncated.
    tokio::time::sleep(DRAIN_BEFORE_CLOSE).await;

    Ok(())
}

/// Builds the catalog once the codec is known and re-publishes it at each group
/// boundary so subscribers that join mid-stream pick it up at the next keyframe.
struct CatalogPublisher {
    writer: TrackWriter,
    namespace: String,
    name: String,
    payload: Option<Bytes>,
}

impl CatalogPublisher {
    fn new(writer: TrackWriter, namespace: String, name: String) -> Self {
        Self {
            writer,
            namespace,
            name,
            payload: None,
        }
    }

    async fn maybe_publish(&mut self, frame: &Frame, codec: &str) -> Result<()> {
        if !frame.keyframe {
            return Ok(());
        }
        if self.payload.is_none() {
            let built = catalog::build_video_catalog(&self.namespace, &self.name, codec);
            self.payload = Some(catalog::serialize(&built)?.into());
            info!(codec = %codec, "codec resolved");
        }
        let payload = self.payload.clone().expect("catalog payload built above");
        self.writer.write_group(payload).await
    }

    async fn finish(self) -> Result<()> {
        self.writer.finish().await
    }
}

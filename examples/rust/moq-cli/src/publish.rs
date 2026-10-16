use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::loc::{CAPTURE_TIMESTAMP_ID, LocExtension, to_extension_headers};
use moqt::{TrackWriter, object::ExtensionHeaders, publish::PublishOption};
use tokio::io::AsyncReadExt;
use tracing::info;

use crate::catalog;
use crate::cli::{Container, PublishArgs, RelayArgs};
use crate::media::{Frame, h264::AnnexBFramer};
use crate::track::FullTrackName;
use crate::transport::{RelayConnection, connect_relay};

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

    let publishing = Publishing::start(&args.relay, &args.track).await?;
    publish_stdin(publishing, &args).await
}

struct Publishing {
    connection: RelayConnection,
    media: TrackWriter,
    catalog: CatalogPublisher,
    /// Whether a media group has started; the stream starts at a keyframe.
    started: bool,
}

impl Publishing {
    async fn start(relay: &RelayArgs, track: &FullTrackName) -> Result<Self> {
        let connection = connect_relay(relay, track.app_id()).await?;
        let publisher = connection.session.publisher();
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
                msf::CATALOG_TRACK_NAME.to_string(),
                PublishOption::default(),
            )
            .await
            .context("failed to publish catalog track")?;
        info!(
            namespace = track.namespace,
            track = track.name,
            "publish ok"
        );

        let first_group_id = unix_micros_now().max(0) as u64;
        let media = TrackWriter::new(publisher.create_stream(&subscription), first_group_id);
        let catalog = CatalogPublisher::new(
            TrackWriter::new(
                publisher.create_stream(&catalog_subscription),
                first_group_id,
            ),
            track.namespace.clone(),
            track.name.clone(),
        );
        Ok(Self {
            connection,
            media,
            catalog,
            started: false,
        })
    }

    async fn write_frame(
        &mut self,
        frame: &Frame,
        codec: &str,
        extension_headers: ExtensionHeaders,
    ) -> Result<()> {
        self.catalog.maybe_publish(frame, codec).await?;
        if frame.keyframe {
            self.media.start_group().await?;
            self.started = true;
        } else if !self.started {
            return Ok(());
        }
        self.media
            .write_with_extension_headers(frame.payload.clone(), extension_headers)
            .await
    }

    /// Ends both tracks and closes the session once the relay has drained
    /// them. Every other handle to the session must be dropped first.
    async fn finish(self) -> Result<()> {
        let groups = self.media.groups();
        self.media.finish().await?;
        self.catalog.finish().await?;
        info!(groups, "tracks finished");
        // Let the relay drain in-flight stream data before the QUIC session is
        // closed; otherwise trailing objects are truncated.
        tokio::time::sleep(DRAIN_BEFORE_CLOSE).await;
        self.connection.close().await;
        Ok(())
    }
}

async fn publish_stdin(mut publishing: Publishing, args: &PublishArgs) -> Result<()> {
    let mut framer = AnnexBFramer::new(args.codec.clone());

    let mut reader = tokio::io::stdin();
    let mut read_buf = vec![0u8; READ_BUFFER_BYTES];
    let mut frame_index: u64 = 0;
    // Wall-clock time of the frame timeline's zero point, set on the first frame.
    // Maps each frame's relative PTS onto a Unix-epoch capture timestamp.
    let mut capture_origin: Option<i64> = None;
    // Keeps the listener registered while the loop body writes frames: tokio
    // drops a SIGINT that arrives while none is registered.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        let session = publishing.connection.session.clone();
        let read = tokio::select! {
            result = reader.read(&mut read_buf) => result?,
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
            _ = &mut ctrl_c => {
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
            let origin =
                *capture_origin.get_or_insert_with(|| unix_micros_now() - frame.timestamp.0);
            let extension_headers = to_extension_headers(&[LocExtension::varint(
                CAPTURE_TIMESTAMP_ID,
                (origin + frame.timestamp.0).max(0) as u64,
            )]);
            publishing
                .write_frame(&frame, &codec, extension_headers)
                .await?;
            frame_index += 1;
        }

        if read == 0 {
            break;
        }
    }

    info!(frames = frame_index, "input ended");
    publishing.finish().await
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

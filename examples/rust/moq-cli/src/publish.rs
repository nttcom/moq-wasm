use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::loc::{CAPTURE_TIMESTAMP_ID, LocExtension, to_extension_headers};
use moqt::{SessionEvent, TrackWriter, publish::PublishOption};
use tokio::{io::AsyncReadExt, task::JoinHandle};
use tracing::info;

use crate::catalog;
use crate::cli::{Container, PublishArgs, RelayArgs};
use crate::media::{Frame, h264::AnnexBFramer};
use crate::track::FullTrackName;
use crate::transport::{MOVE_TIMEOUT, RelayConnection, connect_relay, follow_go_away};

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

    let mut relay = args.relay.clone();
    let publishing = Publishing::start(&relay, &args.track).await?;
    publish_stdin(publishing, &mut relay, &args).await
}

struct Publishing {
    connection: RelayConnection,
    media: TrackWriter,
    catalog: CatalogPublisher,
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
        })
    }

    /// Ends both tracks and closes the session once the relay has drained them.
    async fn finish(self) -> Result<()> {
        let groups = self.media.groups();
        self.media.finish().await?;
        self.catalog.finish().await?;
        info!(groups, "publish done");
        // Let the relay drain in-flight stream data before the QUIC session is
        // closed; otherwise trailing objects are truncated.
        tokio::time::sleep(DRAIN_BEFORE_CLOSE).await;
        self.connection.close().await;
        Ok(())
    }
}

/// draft-14 §8.4.2: on GOAWAY the tracks are published on the relay it names
/// while objects keep going to the current one; once the new relay accepted
/// them, objects go there from the next keyframe on and the old session is
/// withdrawn.
async fn publish_stdin(
    mut publishing: Publishing,
    relay: &mut RelayArgs,
    args: &PublishArgs,
) -> Result<()> {
    let track = &args.track;
    let mut framer = AnnexBFramer::new(args.codec.clone());

    let mut reader = tokio::io::stdin();
    let mut read_buf = vec![0u8; READ_BUFFER_BYTES];
    let mut frame_index: u64 = 0;
    // Wall-clock time of the frame timeline's zero point, set on the first frame.
    // Maps each frame's relative PTS onto a Unix-epoch capture timestamp.
    let mut capture_origin: Option<i64> = None;
    let mut pending_move: Option<JoinHandle<Result<Publishing>>> = None;
    // The new relay's track starts at a keyframe, so frames up to the next
    // one are not sent after a move.
    let mut awaiting_keyframe = false;

    loop {
        let session = publishing.connection.session.clone();
        let read = tokio::select! {
            result = reader.read(&mut read_buf) => result?,
            event = session.receive_event() => {
                if let SessionEvent::GoAway(go_away) = event?
                    && pending_move.is_none()
                {
                    follow_go_away(relay, go_away.new_session_uri())?;
                    info!(relay = %relay.url, "GOAWAY received; publishing on the relay it names");
                    let (relay, track) = (relay.clone(), track.clone());
                    pending_move = Some(tokio::spawn(async move {
                        tokio::time::timeout(MOVE_TIMEOUT, Publishing::start(&relay, &track))
                            .await
                            .context("timed out publishing on the relay named by GOAWAY")?
                    }));
                }
                continue;
            },
            moved = async { pending_move.as_mut().expect("polled only while a move is pending").await },
                if pending_move.is_some() =>
            {
                pending_move = None;
                match moved? {
                    Ok(next) => {
                        let previous = std::mem::replace(&mut publishing, next);
                        awaiting_keyframe = true;
                        if let Err(error) = previous.finish().await {
                            tracing::warn!(%error, "failed to withdraw the tracks from the old relay");
                        }
                        info!(relay = %relay.url, "moved to the relay named by GOAWAY");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to move to the relay named by GOAWAY; staying on the current session");
                    }
                }
                continue;
            },
            close = session.closed() => anyhow::bail!("session closed by the relay: {close}"),
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
            if awaiting_keyframe && !frame.keyframe {
                continue;
            }
            awaiting_keyframe = false;
            let codec = framer
                .codec_string()
                .context("first frame is not a keyframe (no SPS yet)")?;
            publishing.catalog.maybe_publish(&frame, &codec).await?;
            if frame.keyframe {
                publishing.media.start_group().await?;
            }
            let origin =
                *capture_origin.get_or_insert_with(|| unix_micros_now() - frame.timestamp.0);
            let capture_timestamp = LocExtension::varint(
                CAPTURE_TIMESTAMP_ID,
                (origin + frame.timestamp.0).max(0) as u64,
            );
            publishing
                .media
                .write_with_extension_headers(
                    frame.payload,
                    to_extension_headers(&[capture_timestamp]),
                )
                .await?;
            frame_index += 1;
        }

        if read == 0 {
            break;
        }
    }

    if let Some(pending_move) = pending_move {
        pending_move.abort();
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

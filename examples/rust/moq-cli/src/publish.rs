use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::loc::{CAPTURE_TIMESTAMP_ID, LocExtension, to_extension_headers};
use moqt::{SessionEvent, TrackWriter, object::ExtensionHeaders, publish::PublishOption};
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
    /// The PUBLISH request id of each track while the relay subscribes to it.
    media_request_id: Option<u64>,
    catalog_request_id: Option<u64>,
    /// Whether a media group has started on this session; one joined in the
    /// middle of a group starts at the next keyframe.
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
            media_request_id: Some(subscription.request_id()),
            catalog_request_id: Some(catalog_subscription.request_id()),
            started: false,
        })
    }

    /// A session's first groups take the ids in `align`, so that a session
    /// joined after a move numbers the frames as the previous one does.
    async fn write_frame(
        &mut self,
        frame: &Frame,
        codec: &str,
        extension_headers: ExtensionHeaders,
        align: StartedGroups,
    ) -> Result<StartedGroups> {
        let mut started = StartedGroups::default();
        if self.catalog_request_id.is_some() {
            started.catalog = self
                .catalog
                .maybe_publish(frame, codec, align.catalog)
                .await?;
        }
        if self.media_request_id.is_none() {
            return Ok(started);
        }
        if frame.keyframe {
            match align.media.filter(|_| !self.started) {
                Some(group_id) => self.media.start_group_at(group_id).await?,
                None => self.media.start_group().await?,
            }
            self.started = true;
            started.media = self.media.current_group_id();
        } else if !self.started {
            return Ok(started);
        }
        self.media
            .write_with_extension_headers(frame.payload.clone(), extension_headers)
            .await?;
        Ok(started)
    }

    /// Whether the relay still subscribes to a track after unsubscribing from
    /// the one with `request_id`.
    fn unsubscribed(&mut self, request_id: u64) -> bool {
        for subscribed in [&mut self.media_request_id, &mut self.catalog_request_id] {
            if *subscribed == Some(request_id) {
                *subscribed = None;
            }
        }
        self.media_request_id.is_some() || self.catalog_request_id.is_some()
    }

    /// Closes the session without ending the tracks, for a relay that no
    /// longer subscribes to them.
    async fn close(self) {
        drop(self.media);
        drop(self.catalog);
        self.connection.close().await;
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

#[derive(Clone, Copy, Default)]
struct StartedGroups {
    catalog: Option<u64>,
    media: Option<u64>,
}

/// draft-22 §7.6.1: on GOAWAY the tracks are published on the relay it names
/// as well, and every frame goes to both sessions (the new one from its next
/// keyframe) until the old relay has unsubscribed from both tracks or ended
/// the session.
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
    let mut previous: Option<Publishing> = None;

    loop {
        let session = publishing.connection.session.clone();
        let previous_session = previous
            .as_ref()
            .map(|previous| previous.connection.session.clone());
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
                        if let Some(older) = previous.take() {
                            tokio::spawn(older.close());
                        }
                        previous = Some(std::mem::replace(&mut publishing, next));
                        info!(relay = %relay.url, "moved to the relay named by GOAWAY");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to move to the relay named by GOAWAY; staying on the current session");
                    }
                }
                continue;
            },
            event = async { previous_session.as_ref().expect("polled only while the previous session is open").receive_event().await },
                if previous_session.is_some() =>
            {
                let still_subscribed = match event {
                    Ok(SessionEvent::Unsubscribe(handler)) => previous
                        .as_mut()
                        .is_some_and(|previous| previous.unsubscribed(handler.subscribe_id())),
                    Ok(SessionEvent::Disconnected() | SessionEvent::ProtocolViolation()) | Err(_) => false,
                    Ok(_) => true,
                };
                if !still_subscribed && let Some(previous) = previous.take() {
                    drop(previous_session);
                    tokio::spawn(previous.close());
                    info!("closed the previous relay session");
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
            let codec = framer
                .codec_string()
                .context("first frame is not a keyframe (no SPS yet)")?;
            let origin =
                *capture_origin.get_or_insert_with(|| unix_micros_now() - frame.timestamp.0);
            let extension_headers = to_extension_headers(&[LocExtension::varint(
                CAPTURE_TIMESTAMP_ID,
                (origin + frame.timestamp.0).max(0) as u64,
            )]);
            let mut align = StartedGroups::default();
            if let Some(old) = previous.as_mut() {
                match old
                    .write_frame(&frame, &codec, extension_headers.clone(), align)
                    .await
                {
                    Ok(started) => align = started,
                    Err(error) => {
                        tracing::warn!(%error, "the previous relay session stopped taking frames");
                        if let Some(old) = previous.take() {
                            tokio::spawn(old.close());
                        }
                    }
                }
            }
            publishing
                .write_frame(&frame, &codec, extension_headers, align)
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
    if let Some(previous) = previous {
        previous.close().await;
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

    /// Returns the id of the group written; the writer's first group takes
    /// `align` when given.
    async fn maybe_publish(
        &mut self,
        frame: &Frame,
        codec: &str,
        align: Option<u64>,
    ) -> Result<Option<u64>> {
        if !frame.keyframe {
            return Ok(None);
        }
        if self.payload.is_none() {
            let built = catalog::build_video_catalog(&self.namespace, &self.name, codec);
            self.payload = Some(catalog::serialize(&built)?.into());
            info!(codec = %codec, "codec resolved");
        }
        let payload = self.payload.clone().expect("catalog payload built above");
        match align.filter(|_| self.writer.groups() == 0) {
            Some(group_id) => {
                self.writer.start_group_at(group_id).await?;
                self.writer
                    .write_with_extension_headers(payload, ExtensionHeaders::default())
                    .await?;
                Ok(Some(group_id))
            }
            None => {
                let group_id = self.writer.next_group_id();
                self.writer.write_group(payload).await?;
                Ok(Some(group_id))
            }
        }
    }

    async fn finish(self) -> Result<()> {
        self.writer.finish().await
    }
}

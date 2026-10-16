use std::{
    ops::ControlFlow,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{
    Session, Subscription, TrackObject, TrackReader,
    fetch::{Fetch, FetchDataReceiver},
    subscribe::FilterType,
};
use tokio::signal::unix::{Signal, SignalKind, signal};

use crate::{
    Args,
    recording::{Recording, Uploader},
    relay::{self, RelayConnection},
    token_refresh_task::{TokenRefreshTask, read_token_file},
    track_selection::{Selection, select_tracks},
    upload_task::UploadTask,
};

/// Uploads begin when a file closes, so a recording without segments would
/// upload only after it ended.
const UPLOAD_SEGMENT_SECONDS: u64 = 60;

pub(crate) async fn record(args: Args) -> Result<()> {
    let auth_token = match &args.auth_token_file {
        Some(path) => Some(read_token_file(path).await?),
        None => args.auth_token.clone(),
    };
    let connection = RelayConnection::connect(
        &args.relay,
        !args.insecure_skip_tls_verify,
        auth_token.clone(),
    )
    .await?;
    let token_refresh = args
        .auth_token_file
        .clone()
        .zip(auth_token)
        .map(|(path, token)| {
            TokenRefreshTask::run(
                connection.session.clone(),
                path,
                app_id(&args.namespace).to_string(),
                token,
            )
        });
    let result = record_broadcast(&connection.session, &args).await;
    drop(token_refresh);
    connection.close().await;
    result
}

/// An app's namespaces start with its app id, which the relay's token
/// refresh convention names as the TRACK_STATUS namespace.
fn app_id(namespace: &str) -> &str {
    namespace.split('/').next().unwrap_or(namespace)
}

async fn record_broadcast(session: &Session, args: &Args) -> Result<()> {
    let catalog_subscription = relay::subscribe(
        session,
        &args.namespace,
        msf::CATALOG_TRACK_NAME,
        FilterType::NextGroupStart,
    )
    .await?;
    let selection = wait_for_selection(session, &catalog_subscription, args).await?;
    tracing::info!(
        video = %selection.video.name,
        video_codec = ?selection.video.codec,
        audio = selection.audio.as_ref().map(|audio| audio.name.as_str()),
        "catalog resolved"
    );

    let filter_type = if args.from_cache {
        FilterType::LargestObject
    } else {
        FilterType::NextGroupStart
    };
    let video_subscription =
        relay::subscribe(session, &args.namespace, &selection.video.name, filter_type).await?;
    let audio_subscription = match &selection.audio {
        Some(audio) => {
            Some(relay::subscribe(session, &args.namespace, &audio.name, filter_type).await?)
        }
        None => None,
    };
    let video_reader = relay::track_reader(session, &video_subscription).await?;
    let audio_reader = match &audio_subscription {
        Some(subscription) => Some(relay::track_reader(session, subscription).await?),
        None => None,
    };

    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&args.namespace, SystemTime::now()));
    // One listener for the whole recording: tokio drops a SIGINT that arrives
    // while no listener is registered, such as between two `select!` polls.
    let mut interrupt = signal(SignalKind::interrupt())?;
    let segment_seconds = args
        .segment_seconds
        .or(args.upload.is_some().then_some(UPLOAD_SEGMENT_SECONDS));
    let uploader = args
        .upload
        .as_ref()
        .map(|url| uploader(url, &args.namespace, SystemTime::now()))
        .transpose()?;
    let mut recording = Recording::create(
        &selection,
        args.layout,
        path,
        segment_seconds.map(Duration::from_secs),
        uploader,
    )
    .await?;
    tracing::info!(paths = ?recording.paths().collect::<Vec<_>>(), "recording started");
    let cached = if args.from_cache {
        record_cached(
            session,
            &video_subscription,
            audio_subscription.as_ref(),
            &mut recording,
            &mut interrupt,
        )
        .await?
    } else {
        ControlFlow::Continue(())
    };
    if cached.is_continue() {
        record_live(
            session,
            video_reader,
            audio_reader,
            &mut recording,
            &mut interrupt,
        )
        .await?;
    }

    recording.finish().await?;
    tracing::info!("recording finished");
    Ok(())
}

/// Reads the catalog's current object from the joining FETCH
/// (draft-ietf-moq-msf-01 §5), then later objects from the subscription, until
/// the catalog lists the tracks to record. The subscription reader opens only
/// after the FETCH: its first stream arrives with the next catalog object, up
/// to 30 s later.
async fn wait_for_selection(
    session: &Session,
    subscription: &Subscription,
    args: &Args,
) -> Result<Selection> {
    let mut current = match relay::fetch_joining_group(session, subscription).await {
        Ok(current) => current,
        Err(error) => {
            tracing::warn!(%error, "joining FETCH of the catalog failed");
            None
        }
    };
    let mut reader = None;
    loop {
        let payload = match current.take() {
            Some(payload) => payload,
            None => {
                let reader = match &mut reader {
                    Some(reader) => reader,
                    None => reader.insert(relay::track_reader(session, subscription).await?),
                };
                reader
                    .next_object()
                    .await?
                    .context("catalog track ended")?
                    .payload
            }
        };
        let catalog: msf::Catalog =
            serde_json::from_slice(&payload).context("catalog is not MSF JSON")?;
        if let Some(selection) = select_tracks(&catalog, &args.video_track, &args.audio_track)? {
            return Ok(selection);
        }
        tracing::info!(
            track = %args.video_track,
            "waiting for the catalog to list the video track"
        );
    }
}

/// Writes the cached part of both tracks, in arrival order, before the live
/// subscriptions are read; they continue where it ends. A failed FETCH leaves
/// a gap before the live part instead of ending the recording.
async fn record_cached(
    session: &Session,
    video: &Subscription,
    audio: Option<&Subscription>,
    recording: &mut Recording,
    interrupt: &mut Signal,
) -> Result<ControlFlow<()>> {
    let mut video_fetch = open_cached(session, video).await;
    let mut audio_fetch = match audio {
        Some(audio) => open_cached(session, audio).await,
        None => None,
    };
    while video_fetch.is_some() || audio_fetch.is_some() {
        tokio::select! {
            biased;
            _ = interrupt.recv() => {
                tracing::info!("stopping on Ctrl-C");
                return Ok(ControlFlow::Break(()));
            }
            fetched = next_fetched(video_fetch.as_mut()) => {
                if let Some(object) = fetched_object(fetched, "video", &mut video_fetch) {
                    recording.push_video(&object).await?;
                }
            }
            fetched = next_fetched(audio_fetch.as_mut()) => {
                if let Some(object) = fetched_object(fetched, "audio", &mut audio_fetch) {
                    recording.push_audio(&object).await?;
                }
            }
        }
    }
    tracing::info!("cached part recorded; continuing live");
    Ok(ControlFlow::Continue(()))
}

async fn record_live(
    session: &Session,
    mut video_reader: TrackReader,
    mut audio_reader: Option<TrackReader>,
    recording: &mut Recording,
    interrupt: &mut Signal,
) -> Result<()> {
    loop {
        tokio::select! {
            biased;
            _ = interrupt.recv() => {
                tracing::info!("stopping on Ctrl-C");
                return Ok(());
            }
            close = session.closed() => {
                tracing::warn!(%close, "session closed");
                return Ok(());
            }
            object = video_reader.next_object() => match object {
                Ok(Some(object)) => recording.push_video(&object).await?,
                Ok(None) => {
                    tracing::info!("video track ended");
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, "video read error; continuing with the next group");
                }
            },
            object = next_audio_object(audio_reader.as_mut()) => match object {
                Ok(Some(object)) => recording.push_audio(&object).await?,
                Ok(None) => {
                    tracing::info!("audio track ended");
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, "audio read error; continuing with the next group");
                }
            },
        }
    }
}

/// Clears `fetch` once the FETCH ended or failed; status objects yield nothing.
fn fetched_object(
    fetched: Result<Fetch>,
    track: &str,
    fetch: &mut Option<FetchDataReceiver>,
) -> Option<TrackObject> {
    match fetched {
        Ok(Fetch::Object(object)) => relay::fetched_track_object(object),
        Ok(Fetch::End) => {
            *fetch = None;
            None
        }
        Err(error) => {
            tracing::warn!(%error, track, "FETCH of the cached part ended early");
            *fetch = None;
            None
        }
    }
}

async fn open_cached(session: &Session, subscription: &Subscription) -> Option<FetchDataReceiver> {
    relay::fetch_cached(session, subscription)
        .await
        .inspect_err(|error| {
            tracing::warn!(track = subscription.track_name(), %error, "cannot FETCH the cached part")
        })
        .ok()
        .flatten()
}

async fn next_fetched(receiver: Option<&mut FetchDataReceiver>) -> Result<Fetch> {
    match receiver {
        Some(receiver) => receiver.receive().await,
        None => std::future::pending().await,
    }
}

async fn next_audio_object(reader: Option<&mut TrackReader>) -> Result<Option<TrackObject>> {
    match reader {
        Some(reader) => reader.next_object().await,
        None => std::future::pending().await,
    }
}

/// Objects go under `<url path>/<namespace>/`.
fn uploader(url: &url::Url, namespace: &str, now: SystemTime) -> Result<Uploader> {
    let (store, mut prefix) = object_store::parse_url_opts(url, std::env::vars())
        .with_context(|| format!("cannot use {url} as the upload location"))?;
    prefix.extend(namespace.split('/'));
    Ok(Uploader {
        task: UploadTask::run(store.into()),
        prefix,
        started_at_ms: now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    })
}

fn default_output_path(namespace: &str, now: SystemTime) -> PathBuf {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    PathBuf::from(format!("{}-{seconds}.mp4", namespace.replace('/', "_")))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn default_output_path_names_the_namespace_and_the_unix_time() {
        // Arrange
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        // Act
        let path = default_output_path("anon/live/test", now);

        // Assert
        assert_eq!(path, PathBuf::from("anon_live_test-1700000000.mp4"));
    }

    #[tokio::test]
    async fn interrupt_raised_while_not_awaited_is_received_later() {
        // Arrange
        let mut interrupt = signal(SignalKind::interrupt()).unwrap();

        // Act: the runtime handles the SIGINT during the sleep, as it does while `Recording::push_video` awaits
        let status = std::process::Command::new("kill")
            .args(["-INT", &std::process::id().to_string()])
            .status()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let received = tokio::time::timeout(Duration::from_secs(2), interrupt.recv()).await;

        // Assert
        assert!(status.success());
        assert!(received.is_ok(), "the SIGINT was lost");
    }
}

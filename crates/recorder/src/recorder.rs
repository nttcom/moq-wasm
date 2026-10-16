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
use object_store::path::Path;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::watch,
};

use crate::{
    Args,
    catalog_reader::CatalogReader,
    recording::{Layout, Recording, Uploader},
    relay::{self, RelayConnection},
    token_refresh_task::{TokenRefreshTask, read_token_file},
    track_list::track_table,
    track_selection::{Selection, VideoChoice, select_tracks},
    upload_task::{UploadQueue, UploadTask},
};

/// Uploads begin when a file closes, so a recording without segments would
/// upload only after it ended.
const UPLOAD_SEGMENT_SECONDS: u64 = 60;

pub(crate) async fn run(args: Args) -> Result<()> {
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
    let result = if args.list_tracks {
        list_tracks(&connection.session, &args.namespace).await
    } else {
        record_until_interrupted(&connection.session, &args).await
    };
    drop(token_refresh);
    connection.close().await;
    result
}

/// What every broadcast of a recording shares.
pub(crate) struct BroadcastOptions {
    pub(crate) video_track: Option<String>,
    pub(crate) max_video_height: u32,
    pub(crate) audio_track: Option<String>,
    pub(crate) layout: Layout,
    pub(crate) from_cache: bool,
    pub(crate) segment_length: Option<Duration>,
    pub(crate) upload: Option<UploadTarget>,
}

/// Objects go under `prefix`, then the broadcast's namespace.
#[derive(Clone)]
pub(crate) struct UploadTarget {
    pub(crate) queue: UploadQueue,
    pub(crate) prefix: Path,
}

/// Records `args.namespace` until Ctrl-C or the end of a track, then waits
/// for the uploads.
async fn record_until_interrupted(session: &Session, args: &Args) -> Result<()> {
    let upload_task = args
        .upload
        .as_ref()
        .map(|url| {
            let (store, prefix) = object_store::parse_url_opts(url, std::env::vars())
                .with_context(|| format!("cannot use {url} as the upload location"))?;
            anyhow::Ok((UploadTask::run(store.into()), prefix))
        })
        .transpose()?;
    let options = BroadcastOptions {
        video_track: args.video_track.clone(),
        max_video_height: args.max_video_height,
        audio_track: args.audio_track.clone(),
        layout: args.layout,
        from_cache: args.from_cache,
        segment_length: args
            .segment_seconds
            .or(args.upload.is_some().then_some(UPLOAD_SEGMENT_SECONDS))
            .map(Duration::from_secs),
        upload: upload_task.as_ref().map(|(task, prefix)| UploadTarget {
            queue: task.queue(),
            prefix: prefix.clone(),
        }),
    };
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&args.namespace, SystemTime::now()));
    // One listener for the whole recording: tokio drops a SIGINT that arrives
    // while no listener is registered, such as between two `select!` polls.
    let mut interrupt = signal(SignalKind::interrupt())?;
    let (stop_sender, stop) = watch::channel(false);
    let result = {
        let recording = record_broadcast(session, &args.namespace, path, &options, stop);
        tokio::pin!(recording);
        tokio::select! {
            result = &mut recording => result,
            _ = interrupt.recv() => {
                tracing::info!("stopping on Ctrl-C");
                let _ = stop_sender.send(true);
                recording.await
            }
        }
    };
    // The queues the options hold have to go before the upload task can end.
    drop(options);
    if let Some((task, _)) = upload_task {
        tracing::info!("waiting for the uploads to finish");
        task.finish().await;
    }
    result
}

/// An app's namespaces start with its app id, which the relay's token
/// refresh convention names as the TRACK_STATUS namespace.
fn app_id(namespace: &str) -> &str {
    namespace.split('/').next().unwrap_or(namespace)
}

async fn list_tracks(session: &Session, namespace: &str) -> Result<()> {
    let catalog = CatalogReader::open(session, namespace)
        .await?
        .next()
        .await?;
    println!("{}", track_table(&catalog));
    Ok(())
}

/// Records `namespace` into `path` until `stop` turns true or a track ends.
/// A stop while the catalog lists no video track yet records nothing.
pub(crate) async fn record_broadcast(
    session: &Session,
    namespace: &str,
    path: PathBuf,
    options: &BroadcastOptions,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let mut catalog = CatalogReader::open(session, namespace).await?;
    let selection = tokio::select! {
        selection = wait_for_selection(&mut catalog, options) => selection?,
        _ = stop_requested(&mut stop) => return Ok(()),
    };
    tracing::info!(
        video = %selection.video.name,
        video_codec = ?selection.video.codec,
        audio = selection.audio.as_ref().map(|audio| audio.name.as_str()),
        "catalog resolved"
    );

    let filter_type = if options.from_cache {
        FilterType::LargestObject
    } else {
        FilterType::NextGroupStart
    };
    let video_subscription =
        relay::subscribe(session, namespace, &selection.video.name, filter_type).await?;
    let audio_subscription = match &selection.audio {
        Some(audio) => Some(relay::subscribe(session, namespace, &audio.name, filter_type).await?),
        None => None,
    };
    let video_reader = relay::track_reader(session, &video_subscription).await?;
    let audio_reader = match &audio_subscription {
        Some(subscription) => Some(relay::track_reader(session, subscription).await?),
        None => None,
    };

    let uploader = options
        .upload
        .as_ref()
        .map(|target| uploader(target, namespace, SystemTime::now()));
    let mut recording = Recording::create(
        &selection,
        options.layout,
        path,
        options.segment_length,
        uploader,
    )
    .await?;
    tracing::info!(paths = ?recording.paths().collect::<Vec<_>>(), "recording started");
    let cached = if options.from_cache {
        record_cached(
            session,
            &video_subscription,
            audio_subscription.as_ref(),
            &mut recording,
            &mut stop,
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
            &mut stop,
        )
        .await?;
    }

    recording.finish().await?;
    tracing::info!("recording finished");
    Ok(())
}

async fn wait_for_selection(
    catalog: &mut CatalogReader<'_>,
    options: &BroadcastOptions,
) -> Result<Selection> {
    let video_choice = match &options.video_track {
        Some(name) => VideoChoice::Named(name),
        None => VideoChoice::MaxHeight(options.max_video_height),
    };
    loop {
        let selection = select_tracks(
            &catalog.next().await?,
            video_choice,
            options.audio_track.as_deref(),
        )?;
        if let Some(selection) = selection {
            return Ok(selection);
        }
        tracing::info!(
            track = options.video_track.as_deref(),
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
    stop: &mut watch::Receiver<bool>,
) -> Result<ControlFlow<()>> {
    let mut video_fetch = open_cached(session, video).await;
    let mut audio_fetch = match audio {
        Some(audio) => open_cached(session, audio).await,
        None => None,
    };
    while video_fetch.is_some() || audio_fetch.is_some() {
        tokio::select! {
            biased;
            _ = stop_requested(stop) => {
                tracing::info!("stopping");
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
    stop: &mut watch::Receiver<bool>,
) -> Result<()> {
    loop {
        tokio::select! {
            biased;
            _ = stop_requested(stop) => {
                tracing::info!("stopping");
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

/// Resolves once `stop` turns true, or at once when its sender is gone.
async fn stop_requested(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

fn uploader(target: &UploadTarget, namespace: &str, now: SystemTime) -> Uploader {
    let mut prefix = target.prefix.clone();
    prefix.extend(namespace.split('/'));
    Uploader {
        queue: target.queue.clone(),
        prefix,
        started_at_ms: now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    }
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

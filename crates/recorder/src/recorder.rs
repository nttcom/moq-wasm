use std::{
    ops::ControlFlow,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use mediapack::Timestamp;
use moqt::{
    Session, TrackObject,
    fetch::{Fetch, FetchDataReceiver},
    subscribe::FilterType,
};
use msf::Catalog;
use object_store::path::Path;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::watch,
};

use crate::{
    Args,
    catalog_reader::CatalogReader,
    recording::{Layout, RecordedFile, Recording, Uploader},
    relay::{self, RelayConnection, Track},
    token_refresh_task::{TokenRefreshTask, read_token_file},
    track_list::track_table,
    track_selection::{
        Selection, VideoChoice, loc_tracks, select_audio, select_tracks, select_video,
    },
    upload_task::{UploadQueue, UploadTask},
};

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
    let session = &connection.session;
    let token_refresh = args
        .auth_token_file
        .clone()
        .zip(auth_token)
        .map(|(path, token)| {
            TokenRefreshTask::run(
                session.clone(),
                path,
                app_id(&args.namespace).to_string(),
                token,
            )
        });
    let result = if args.list_tracks {
        list_tracks(session, &args.namespace).await
    } else {
        record_until_interrupted(session, &args).await
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

/// Records `args.namespace` until Ctrl-C or the end of the broadcast, then
/// waits for the uploads.
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
        segment_length: args.segment_seconds.map(Duration::from_secs),
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
    result.map(drop)
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
        .await?
        .context("catalog track ended")?;
    println!("{}", track_table(&catalog));
    Ok(())
}

/// Records `namespace` into `path` until `stop` turns true, or until the
/// catalog and every media track read have ended. A stop before the catalog
/// lists a track records nothing. A track the catalog unlists, or one that
/// ends, is dropped; a later catalog that lists one adds it again. Returns
/// every file written, in closing order.
pub(crate) async fn record_broadcast(
    session: &Session,
    namespace: &str,
    path: PathBuf,
    options: &BroadcastOptions,
    mut stop: watch::Receiver<bool>,
) -> Result<Vec<RecordedFile>> {
    let mut catalog = CatalogReader::open(session, namespace).await?;
    let selection = tokio::select! {
        selection = wait_for_selection(&mut catalog, options) => selection?,
        _ = stop_requested(&mut stop) => return Ok(Vec::new()),
    };
    tracing::info!(
        video = selection.video.as_ref().map(|video| video.name.as_str()),
        video_codec = ?selection.video.as_ref().map(|video| video.codec),
        audio = selection.audio.as_ref().map(|audio| audio.name.as_str()),
        "catalog resolved"
    );
    if selection.audio.is_none() {
        tracing::info!(
            track = options.audio_track.as_deref(),
            "catalog lists no LOC audio track that can be decoded yet; recording video until one is listed"
        );
    }

    let filter_type = if options.from_cache {
        FilterType::LargestObject
    } else {
        FilterType::NextGroupStart
    };
    let video = match &selection.video {
        Some(video) => Some(relay::subscribe(session, namespace, &video.name, filter_type).await?),
        None => None,
    };
    let audio = match &selection.audio {
        Some(audio) => Some(relay::subscribe(session, namespace, &audio.name, filter_type).await?),
        None => None,
    };
    let mut tracks = Subscriptions {
        catalog,
        video,
        audio,
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
            tracks.video.as_ref(),
            tracks.audio.as_ref(),
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
            namespace,
            &mut tracks,
            options,
            &mut recording,
            &mut stop,
        )
        .await?;
    }

    let files = recording.finish().await?;
    for file in &files {
        tracing::info!(
            path = %file.path.display(),
            capture_origin_us = file.capture_origin.map(Timestamp::micros),
            "recorded file closed"
        );
    }
    tracing::info!("recording finished");
    Ok(files)
}

async fn wait_for_selection(
    catalog: &mut CatalogReader,
    options: &BroadcastOptions,
) -> Result<Selection> {
    loop {
        let selection = select_tracks(
            &catalog.next().await?.context("catalog track ended")?,
            video_choice(options),
            options.audio_track.as_deref(),
        )?;
        if let Some(selection) = selection {
            return Ok(selection);
        }
        tracing::info!(
            video = options.video_track.as_deref(),
            audio = options.audio_track.as_deref(),
            "waiting for the catalog to list a video or an audio track"
        );
    }
}

fn video_choice(options: &BroadcastOptions) -> VideoChoice<'_> {
    match &options.video_track {
        Some(name) => VideoChoice::Named(name),
        None => VideoChoice::MaxHeight(options.max_video_height),
    }
}

/// Writes the cached part of both tracks, in arrival order, before the live
/// subscriptions are read; they continue where it ends. A failed FETCH leaves
/// a gap before the live part instead of ending the recording.
async fn record_cached(
    session: &Session,
    video: Option<&Track>,
    audio: Option<&Track>,
    recording: &mut Recording,
    stop: &mut watch::Receiver<bool>,
) -> Result<ControlFlow<()>> {
    let mut video_fetch = open_cached(session, video).await;
    let mut audio_fetch = open_cached(session, audio).await;
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

struct Subscriptions {
    catalog: CatalogReader,
    video: Option<Track>,
    audio: Option<Track>,
}

async fn record_live(
    session: &Session,
    namespace: &str,
    tracks: &mut Subscriptions,
    options: &BroadcastOptions,
    recording: &mut Recording,
    stop: &mut watch::Receiver<bool>,
) -> Result<()> {
    let mut catalog_open = true;
    while catalog_open || tracks.video.is_some() || tracks.audio.is_some() {
        // `biased` polls the catalog first: a track listed while the other
        // track's first object is ready joins the file before that object is
        // written.
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
            catalog = tracks.catalog.next(), if catalog_open => match catalog {
                Ok(Some(catalog)) => follow_catalog(session, namespace, &catalog, tracks, options, recording).await?,
                Ok(None) => catalog_open = false,
                Err(error) => tracing::warn!(%error, "catalog read error"),
            },
            object = next_object(tracks.video.as_mut()) => match object {
                Ok(Some(object)) => recording.push_video(&object).await?,
                Ok(None) => {
                    tracing::info!("video track ended; recording audio until the catalog lists one");
                    tracks.video = None;
                    recording.end_video().await?;
                }
                Err(error) => {
                    tracing::warn!(%error, "video read error; continuing with the next group");
                }
            },
            object = next_object(tracks.audio.as_mut()) => match object {
                Ok(Some(object)) => recording.push_audio(&object).await?,
                Ok(None) => {
                    tracing::info!("audio track ended; recording video until the catalog lists one");
                    tracks.audio = None;
                }
                Err(error) => {
                    tracing::warn!(%error, "audio read error; continuing with the next group");
                }
            },
        }
    }
    tracing::info!("catalog and media tracks ended");
    Ok(())
}

/// Drops each track `catalog` no longer lists, then adds a listed video or
/// audio track where the recording reads none. A meeting participant who
/// mutes or turns the camera off unlists the track before sending
/// PUBLISH_DONE, and `moqt` reports the track end 10 s later; without the
/// drop, turning it back on within those 10 s would go unnoticed.
async fn follow_catalog(
    session: &Session,
    namespace: &str,
    catalog: &Catalog,
    tracks: &mut Subscriptions,
    options: &BroadcastOptions,
    recording: &mut Recording,
) -> Result<()> {
    if tracks
        .video
        .as_ref()
        .is_some_and(|video| !lists(catalog, video))
    {
        tracing::info!(
            "catalog no longer lists the video track; recording audio until it lists one"
        );
        tracks.video = None;
        recording.end_video().await?;
    }
    if tracks
        .audio
        .as_ref()
        .is_some_and(|audio| !lists(catalog, audio))
    {
        tracing::info!(
            "catalog no longer lists the audio track; recording video until it lists one"
        );
        tracks.audio = None;
    }
    if tracks.video.is_none() {
        match add_listed_video(session, namespace, catalog, options, recording).await {
            Ok(video) => tracks.video = video,
            Err(error) => tracing::warn!(%error, "cannot add the video track"),
        }
    }
    if tracks.audio.is_none() {
        match add_listed_audio(session, namespace, catalog, options, recording).await {
            Ok(audio) => tracks.audio = audio,
            Err(error) => tracing::warn!(%error, "cannot add the audio track"),
        }
    }
    Ok(())
}

fn lists(catalog: &Catalog, track: &Track) -> bool {
    let name = track.subscription().track_name();
    loc_tracks(catalog).any(|listed| listed.name == name)
}

async fn add_listed_video(
    session: &Session,
    namespace: &str,
    catalog: &Catalog,
    options: &BroadcastOptions,
    recording: &mut Recording,
) -> Result<Option<Track>> {
    let Some(video) = select_video(catalog, video_choice(options))? else {
        return Ok(None);
    };
    let track =
        relay::subscribe(session, namespace, &video.name, FilterType::NextGroupStart).await?;
    recording.add_video(&video).await?;
    tracing::info!(video = %video.name, "catalog lists the video track now; recording it");
    Ok(Some(track))
}

async fn add_listed_audio(
    session: &Session,
    namespace: &str,
    catalog: &Catalog,
    options: &BroadcastOptions,
    recording: &mut Recording,
) -> Result<Option<Track>> {
    let Some(audio) = select_audio(catalog, options.audio_track.as_deref())? else {
        return Ok(None);
    };
    let track =
        relay::subscribe(session, namespace, &audio.name, FilterType::NextGroupStart).await?;
    recording.add_audio(&audio).await?;
    tracing::info!(audio = %audio.name, "catalog lists the audio track now; recording it");
    Ok(Some(track))
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

async fn open_cached(session: &Session, track: Option<&Track>) -> Option<FetchDataReceiver> {
    let track = track?;
    relay::fetch_cached(session, track)
        .await
        .inspect_err(|error| {
            tracing::warn!(track = track.subscription().track_name(), %error, "cannot FETCH the cached part")
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

async fn next_object(track: Option<&mut Track>) -> Result<Option<TrackObject>> {
    match track {
        Some(track) => track.next_object().await,
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

pub(crate) fn default_output_path(namespace: &str, now: SystemTime) -> PathBuf {
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
